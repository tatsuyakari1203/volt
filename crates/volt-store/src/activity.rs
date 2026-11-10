//! [`ActivityLog`] port on `SQLite`.

use rusqlite::{Row, params};
use volt_core::{
    ActivityId, ActivityLog, ActivityOutcome, ActivityRecord, BatchId, EntryId, NewActivity,
    PortError, Source, Timestamp,
};

use crate::journal::{decode_unit, encode_unit, to_i64, to_u64};
use crate::store::{Store, db_error};

const COLUMNS: &str = "id, timestamp_ms, source, outcome, summary, batch, entry, label";

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
        })
    }
}

impl ActivityLog for Store {
    fn record(&self, activity: NewActivity) -> Result<ActivityId, PortError> {
        let source = encode_unit(&activity.source)?;
        let outcome = encode_unit(&activity.outcome)?;
        let batch = activity.batch.map(|b| to_i64(b.0)).transpose()?;
        let entry = activity.entry.map(|e| to_i64(e.0)).transpose()?;
        let conn = self.conn();
        conn.execute(
            "INSERT INTO activity \
             (timestamp_ms, source, outcome, summary, batch, entry, label) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                activity.timestamp.as_unix_millis(),
                source,
                outcome,
                activity.summary,
                batch,
                entry,
                activity.label
            ],
        )
        .map_err(|e| db_error(&e))?;
        Ok(ActivityId(to_u64(conn.last_insert_rowid())?))
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
