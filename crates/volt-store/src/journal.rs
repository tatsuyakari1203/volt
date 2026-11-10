//! [`Journal`] port on `SQLite`.

use rusqlite::{OptionalExtension, Row, params};
use serde::Serialize;
use serde::de::DeserializeOwned;
use volt_core::{
    BatchId, EntryId, EntryState, Journal, JournalEntry, NewEntry, Operation, PortError,
    PriorState, Source, Timestamp,
};

use crate::store::{Store, db_error};

const COLUMNS: &str = "id, batch, timestamp_ms, source, state, operation, prior, error";

/// A row as stored, before decoding.
struct RawEntry {
    id: i64,
    batch: i64,
    timestamp_ms: i64,
    source: String,
    state: String,
    operation: String,
    prior: String,
    error: Option<String>,
}

impl RawEntry {
    fn read(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            batch: row.get(1)?,
            timestamp_ms: row.get(2)?,
            source: row.get(3)?,
            state: row.get(4)?,
            operation: row.get(5)?,
            prior: row.get(6)?,
            error: row.get(7)?,
        })
    }

    fn decode(self) -> Result<JournalEntry, PortError> {
        Ok(JournalEntry {
            id: EntryId(to_u64(self.id)?),
            batch: BatchId(to_u64(self.batch)?),
            timestamp: Timestamp::from_unix_millis(self.timestamp_ms),
            source: decode_unit::<Source>(&self.source)?,
            state: decode_unit::<EntryState>(&self.state)?,
            operation: decode_json::<Operation>(&self.operation)?,
            prior: decode_json::<PriorState>(&self.prior)?,
            error: self.error,
        })
    }
}

fn to_i64(value: u64) -> Result<i64, PortError> {
    i64::try_from(value).map_err(|_| PortError::InvalidInput(format!("id {value} out of range")))
}

fn to_u64(value: i64) -> Result<u64, PortError> {
    u64::try_from(value)
        .map_err(|_| PortError::Other(format!("journal holds a negative id {value}")))
}

fn encode_json<T: Serialize>(value: &T) -> Result<String, PortError> {
    serde_json::to_string(value).map_err(|e| PortError::Other(format!("cannot encode entry: {e}")))
}

fn decode_json<T: DeserializeOwned>(text: &str) -> Result<T, PortError> {
    serde_json::from_str(text)
        .map_err(|e| PortError::Other(format!("journal row is unreadable: {e}")))
}

/// Unit-variant enums are stored as their bare name, not as a JSON string.
fn encode_unit<T: Serialize>(value: &T) -> Result<String, PortError> {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(name)) => Ok(name),
        Ok(other) => Err(PortError::Other(format!("unexpected encoding {other}"))),
        Err(e) => Err(PortError::Other(format!("cannot encode value: {e}"))),
    }
}

fn decode_unit<T: DeserializeOwned>(name: &str) -> Result<T, PortError> {
    decode_json(&serde_json::Value::String(name.to_owned()).to_string())
}

impl Journal for Store {
    fn new_batch(&self) -> Result<BatchId, PortError> {
        let now = self.clock.now().as_unix_millis();
        let conn = self.conn();
        conn.execute("INSERT INTO batches (created_ms) VALUES (?1)", [now])
            .map_err(|e| db_error(&e))?;
        Ok(BatchId(to_u64(conn.last_insert_rowid())?))
    }

    fn record_pending(&self, entry: NewEntry) -> Result<EntryId, PortError> {
        let operation = encode_json(&entry.operation)?;
        let prior = encode_json(&entry.prior)?;
        let source = encode_unit(&entry.source)?;
        let state = encode_unit(&EntryState::Pending)?;
        let batch = to_i64(entry.batch.0)?;
        let conn = self.conn();
        // One auto-committed statement: with WAL and synchronous=FULL the
        // commit is fsynced before this returns, so the prior state is on disk
        // before the executor mutates anything.
        conn.execute(
            "INSERT INTO journal_entries \
             (batch, timestamp_ms, source, state, operation, prior, error) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL)",
            params![
                batch,
                entry.timestamp.as_unix_millis(),
                source,
                state,
                operation,
                prior
            ],
        )
        .map_err(|e| db_error(&e))?;
        Ok(EntryId(to_u64(conn.last_insert_rowid())?))
    }

    fn set_state(
        &self,
        id: EntryId,
        state: EntryState,
        error: Option<String>,
    ) -> Result<(), PortError> {
        let state = encode_unit(&state)?;
        let changed = self
            .conn()
            .execute(
                "UPDATE journal_entries SET state = ?1, error = ?2 WHERE id = ?3",
                params![state, error, to_i64(id.0)?],
            )
            .map_err(|e| db_error(&e))?;
        if changed == 0 {
            return Err(PortError::NotFound(format!("journal entry {id}")));
        }
        Ok(())
    }

    fn entry(&self, id: EntryId) -> Result<Option<JournalEntry>, PortError> {
        let raw = self
            .conn()
            .query_row(
                &format!("SELECT {COLUMNS} FROM journal_entries WHERE id = ?1"),
                [to_i64(id.0)?],
                RawEntry::read,
            )
            .optional()
            .map_err(|e| db_error(&e))?;
        raw.map(RawEntry::decode).transpose()
    }

    fn batch_entries(&self, batch: BatchId) -> Result<Vec<JournalEntry>, PortError> {
        self.query_entries(
            &format!("SELECT {COLUMNS} FROM journal_entries WHERE batch = ?1 ORDER BY id"),
            to_i64(batch.0)?,
        )
    }

    fn entries_in_state(&self, state: EntryState) -> Result<Vec<JournalEntry>, PortError> {
        self.query_entries(
            &format!("SELECT {COLUMNS} FROM journal_entries WHERE state = ?1 ORDER BY id"),
            encode_unit(&state)?,
        )
    }
}

impl Store {
    fn query_entries(
        &self,
        sql: &str,
        param: impl rusqlite::ToSql,
    ) -> Result<Vec<JournalEntry>, PortError> {
        let conn = self.conn();
        let mut statement = conn.prepare(sql).map_err(|e| db_error(&e))?;
        let raws = statement
            .query_map([param], RawEntry::read)
            .map_err(|e| db_error(&e))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| db_error(&e))?;
        raws.into_iter().map(RawEntry::decode).collect()
    }
}
