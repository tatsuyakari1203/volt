//! [`Settings`] port on `SQLite`.

use rusqlite::OptionalExtension;
use volt_core::{PortError, Settings};

use crate::store::{Store, db_error};

impl Settings for Store {
    fn get(&self, key: &str) -> Result<Option<serde_json::Value>, PortError> {
        let text: Option<String> = self
            .conn()
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()
            .map_err(|e| db_error(&e))?;
        text.map(|text| {
            serde_json::from_str(&text)
                .map_err(|e| PortError::Other(format!("setting {key} is unreadable: {e}")))
        })
        .transpose()
    }

    fn set(&self, key: &str, value: &serde_json::Value) -> Result<(), PortError> {
        self.conn()
            .execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2) \
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [key, &value.to_string()],
            )
            .map_err(|e| db_error(&e))?;
        Ok(())
    }

    fn remove(&self, key: &str) -> Result<bool, PortError> {
        let changed = self
            .conn()
            .execute("DELETE FROM settings WHERE key = ?1", [key])
            .map_err(|e| db_error(&e))?;
        Ok(changed > 0)
    }
}
