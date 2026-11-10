//! Port: small persistent key-value settings.
//!
//! Values are JSON so the store stays schema-free and callers stay typed
//! through [`SettingsExt`].

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::PortError;

/// Port: persistent user settings, a map from key to JSON value.
pub trait Settings: Send + Sync {
    /// Reads a value. Missing key is `Ok(None)`.
    fn get(&self, key: &str) -> Result<Option<serde_json::Value>, PortError>;

    /// Stores a value, replacing any existing one. Durable when it returns.
    fn set(&self, key: &str, value: &serde_json::Value) -> Result<(), PortError>;

    /// Removes a key. Returns whether it existed.
    fn remove(&self, key: &str) -> Result<bool, PortError>;
}

/// Typed access on top of any [`Settings`].
pub trait SettingsExt: Settings {
    /// Reads and deserializes a value. A stored value of the wrong shape is
    /// [`PortError::InvalidInput`].
    fn get_typed<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>, PortError> {
        match self.get(key)? {
            None => Ok(None),
            Some(value) => serde_json::from_value(value)
                .map(Some)
                .map_err(|e| PortError::InvalidInput(format!("setting {key}: {e}"))),
        }
    }

    /// Serializes and stores a value.
    fn set_typed<T: Serialize>(&self, key: &str, value: &T) -> Result<(), PortError> {
        let value = serde_json::to_value(value)
            .map_err(|e| PortError::InvalidInput(format!("setting {key}: {e}")))?;
        self.set(key, &value)
    }
}

impl<S: Settings + ?Sized> SettingsExt for S {}
