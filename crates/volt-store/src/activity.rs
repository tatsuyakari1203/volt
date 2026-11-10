//! [`ActivityLog`] port on `SQLite`.

use rusqlite::{Connection, Row, params};
use volt_core::{
    ActivityId, ActivityLog, ActivityOutcome, ActivityRecord, BatchId, EntryId, NewActivity,
    Phrase, PortError, Retention, Source, Timestamp,
};

use crate::journal::{decode_unit, encode_unit, to_i64, to_u64};
use crate::store::{Store, db_error};

const COLUMNS: &str = "id, timestamp_ms, source, outcome, summary, batch, entry, label, phrase";

/// Write count between two size checks of the log.
const PRUNE_EVERY: i64 = 100;

/// Records an undo can still reach: linked to a journal entry that is applied (or pending). They
/// are never pruned or cleared, because the Undo page lists those changes from the log.
const NOT_UNDOABLE: &str = "(entry IS NULL OR entry NOT IN      (SELECT id FROM journal_entries WHERE state IN ('Applied', 'Pending')))";

/// Deletes records older than the age limit or beyond the newest `max_records`, except the ones an
/// undo can still reach. Only the `activity` table is written.
fn prune_in(conn: &Connection, retention: Retention, now: Timestamp) -> Result<u64, PortError> {
    let cutoff = now.as_unix_millis().saturating_sub(retention.max_age_ms);
    let keep = i64::try_from(retention.max_records).unwrap_or(i64::MAX);
    let removed = conn
        .execute(
            &format!(
                "DELETE FROM activity WHERE {NOT_UNDOABLE} AND (timestamp_ms < ?1 OR id <=                  COALESCE((SELECT id FROM activity ORDER BY id DESC LIMIT 1 OFFSET ?2), 0))"
            ),
            params![cutoff, keep],
        )
        .map_err(|e| db_error(&e))?;
    Ok(removed as u64)
}

/// A row as stored, before decoding.
struct RawRecord {
    id: i64,
    timestamp_ms: i64,
    source: String,
    outcome: String,
    summary: String,
    batch: Option<i64>,
    entry: Option<i64>,
    label: Option<String>,
    phrase: Option<String>,
}

impl RawRecord {
    fn read(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            timestamp_ms: row.get(1)?,
            source: row.get(2)?,
            outcome: row.get(3)?,
            summary: row.get(4)?,
            batch: row.get(5)?,
            entry: row.get(6)?,
            label: row.get(7)?,
            phrase: row.get(8)?,
        })
    }

    fn decode(self) -> Result<ActivityRecord, PortError> {
        Ok(ActivityRecord {
            id: ActivityId(to_u64(self.id)?),
            timestamp: Timestamp::from_unix_millis(self.timestamp_ms),
            source: decode_unit::<Source>(&self.source)?,
            outcome: decode_unit::<ActivityOutcome>(&self.outcome)?,
            summary: self.summary,
            batch: self.batch.map(|b| to_u64(b).map(BatchId)).transpose()?,
            entry: self.entry.map(|e| to_u64(e).map(EntryId)).transpose()?,
            label: self.label,
            // A phrase that cannot be read is no reason to lose the record: its summary stands.
            phrase: self
                .phrase
                .and_then(|text| serde_json::from_str::<Phrase>(&text).ok()),
        })
    }
}

impl ActivityLog for Store {
    fn record(&self, activity: NewActivity) -> Result<ActivityId, PortError> {
        let source = encode_unit(&activity.source)?;
        let outcome = encode_unit(&activity.outcome)?;
        let batch = activity.batch.map(|b| to_i64(b.0)).transpose()?;
        let entry = activity.entry.map(|e| to_i64(e.0)).transpose()?;
        let phrase = activity
            .phrase
            .as_ref()
            .map(|phrase| {
                serde_json::to_string(phrase).map_err(|error| PortError::Other(error.to_string()))
            })
            .transpose()?;
        let conn = self.conn();
        conn.execute(
            "INSERT INTO activity \
             (timestamp_ms, source, outcome, summary, batch, entry, label, phrase) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                activity.timestamp.as_unix_millis(),
                source,
                outcome,
                activity.summary,
                batch,
                entry,
                activity.label,
                phrase
            ],
        )
        .map_err(|e| db_error(&e))?;
        let id = conn.last_insert_rowid();
        // Cheap upkeep: look at the size once every PRUNE_EVERY writes.
        if id % PRUNE_EVERY == 0
            && let Err(error) = prune_in(&conn, Retention::DEFAULT, activity.timestamp)
        {
            log::warn!("pruning the activity log failed: {error}");
        }
        Ok(ActivityId(to_u64(id)?))
    }

    fn prune(&self, retention: Retention, now: Timestamp) -> Result<u64, PortError> {
        prune_in(&self.conn(), retention, now)
    }

    fn clear(&self) -> Result<u64, PortError> {
        let conn = self.conn();
        let removed = conn
            .execute(&format!("DELETE FROM activity WHERE {NOT_UNDOABLE}"), [])
            .map_err(|e| db_error(&e))?;
        Ok(removed as u64)
    }

    fn list(
        &self,
        limit: usize,
        before: Option<ActivityId>,
    ) -> Result<Vec<ActivityRecord>, PortError> {
        // `i64::MAX` is "no cursor": ids are positive and never reach it.
        let before = match before {
            Some(cursor) => to_i64(cursor.0)?,
            None => i64::MAX,
        };
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let conn = self.conn();
        let mut statement = conn
            .prepare(&format!(
                "SELECT {COLUMNS} FROM activity WHERE id < ?1 ORDER BY id DESC LIMIT ?2"
            ))
            .map_err(|e| db_error(&e))?;
        let raws = statement
            .query_map(params![before, limit], RawRecord::read)
            .map_err(|e| db_error(&e))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| db_error(&e))?;
        raws.into_iter().map(RawRecord::decode).collect()
    }
}
