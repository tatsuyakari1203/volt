//! The activity log: what Volt did, for the user (and the Undo page).
//!
//! It is a history written for people, separate from the undo journal. The
//! journal holds what revert needs (prior values, in full) and is the source
//! of truth; an activity record is a short, human-readable sentence about one
//! event, linked to the journal batch and entry when there is one. Events that
//! are not journaled operations (cleaning, memory purges) can be recorded too,
//! so the log is not a view over the journal.
//!
//! # Redaction rule
//!
//! A summary names the *target* (registry key and value name, service name,
//! the program of a command) and the *outcome*. It never contains
//!
//! - registry value data of string or binary kinds: only the kind and the
//!   length are shown (numbers of the `DWORD` and `QWORD` kinds are shown),
//! - command arguments or command output,
//! - file contents,
//! - the text of an error a port produced (it can carry command output or
//!   data): only the error class, such as "access denied".
//!
//! The same rule applies to the diagnostic log. Anything written into a
//! summary by code outside this module must follow it too.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::{ExecError, PortError, RevertError};
use crate::journal::{BatchId, EntryId, Source};
use crate::operation::{Operation, PriorState};
use crate::registry::RegistryValue;
use crate::startup::{StartupState, approved_entry_name, decode_approval};
use crate::time::Timestamp;

/// Identifier of an activity record, assigned by the activity log. Ids
/// increase with write order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ActivityId(pub u64);

impl fmt::Display for ActivityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "activity {}", self.0)
    }
}

/// How the recorded event ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ActivityOutcome {
    /// A change was made.
    Applied,
    /// A change was undone on request.
    Reverted,
    /// An attempt failed (or was found to have been interrupted).
    Failed,
    /// A change was undone automatically because a later step of the same
    /// plan failed.
    RolledBack,
}

impl fmt::Display for ActivityOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Applied => "applied",
            Self::Reverted => "reverted",
            Self::Failed => "failed",
            Self::RolledBack => "rolled back",
        })
    }
}

/// An event about to be written to the activity log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewActivity {
    /// When it happened, from the injected clock.
    pub timestamp: Timestamp,
    /// Who started it.
    pub source: Source,
    /// How it ended.
    pub outcome: ActivityOutcome,
    /// One human-readable line. Must follow the redaction rule in the
    /// [module documentation](self).
    pub summary: String,
    /// The journal batch, when the event belongs to one.
    pub batch: Option<BatchId>,
    /// The journal entry, when the event is about one.
    pub entry: Option<EntryId>,
    /// The tweak id or plan label, when known.
    pub label: Option<String>,
}

/// A stored activity record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityRecord {
    /// Record id.
    pub id: ActivityId,
    /// When it happened.
    pub timestamp: Timestamp,
    /// Who started it.
    pub source: Source,
    /// How it ended.
    pub outcome: ActivityOutcome,
    /// One human-readable line.
    pub summary: String,
    /// The journal batch, when the event belongs to one.
    pub batch: Option<BatchId>,
    /// The journal entry, when the event is about one.
    pub entry: Option<EntryId>,
    /// The tweak id or plan label, when known.
    pub label: Option<String>,
}

/// Port: the persistent activity log.
pub trait ActivityLog: Send + Sync {
    /// Appends a record and returns its id. Ids increase with write order.
    fn record(&self, activity: NewActivity) -> Result<ActivityId, PortError>;

    /// Reads records newest first (by id), at most `limit` of them. With
    /// `before`, only records with a smaller id are returned, so the id of the
    /// last record of a page is the cursor for the next page. A `limit` of
    /// zero returns nothing.
    fn list(
        &self,
        limit: usize,
        before: Option<ActivityId>,
    ) -> Result<Vec<ActivityRecord>, PortError>;

    /// Removes records that are older than `retention.max_age_ms` before `now`, or beyond the
    /// newest `retention.max_records`. Returns how many were removed. Touches the activity log
    /// only, and keeps every record linked to a journal entry that can still be undone (the
    /// Undo page lists those changes from the log).
    fn prune(&self, retention: Retention, now: Timestamp) -> Result<u64, PortError>;

    /// Removes every record except those linked to a journal entry that can still be undone.
    /// Returns how many were removed. Never touches the journal.
    fn clear(&self) -> Result<u64, PortError>;
}

/// How much activity history is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Retention {
    /// Records older than this many milliseconds are removed.
    pub max_age_ms: i64,
    /// Only this many of the newest records are kept.
    pub max_records: usize,
}

impl Retention {
    /// The fixed default: 90 days and 5000 records, whichever removes more.
    pub const DEFAULT: Self = Self {
        max_age_ms: 90 * 24 * 60 * 60 * 1000,
        max_records: 5000,
    };
}

/// The journal's failure text for an entry, redacted for display or export. Only the error class
/// is ever shown: a command's output stored by an older build in the entry error is dropped.
#[must_use]
pub fn redacted_entry_error(entry: &crate::journal::JournalEntry) -> Option<String> {
    entry.error.as_deref().map(|stored| {
        const CLASSES: [&str; 5] = [
            "not found",
            "access denied",
            "unsupported",
            "invalid input",
            "error",
        ];
        CLASSES
            .iter()
            .find(|class| **class == stored)
            .map_or("error", |class| class)
            .to_owned()
    })
}

/// Renders records as plain text, one line per record, in the order given
/// (pass [`ActivityLog::list`] output for newest first).
///
/// A line is the UTC time in ISO 8601, the source, the outcome and the
/// summary, then the links in brackets when there are any:
///
/// ```text
/// 2026-11-10T03:01:00.123Z manual     applied     Applied registry value HKCU\Software\X\Flag = REG_DWORD 1 [label tweak.x, batch 3, entry 7]
/// ```
///
/// The text is stable (the same records always give the same text) and holds
/// only what the records hold, so it follows their redaction rule. Control
/// characters in a summary or label become spaces so one record is one line.
#[must_use]
pub fn render_text(records: &[ActivityRecord]) -> String {
    let mut text = String::new();
    for record in records {
        let mut line = format!(
            "{} {:<10} {:<11} {}",
            record.timestamp.to_iso8601(),
            record.source.to_string(),
            record.outcome.to_string(),
            one_line(&record.summary),
        );
        let mut links = Vec::new();
        if let Some(label) = &record.label {
            links.push(format!("label {}", one_line(label)));
        }
        if let Some(batch) = record.batch {
            links.push(format!("batch {}", batch.0));
        }
        if let Some(entry) = record.entry {
            links.push(format!("entry {}", entry.0));
        }
        if !links.is_empty() {
            line = format!("{line} [{}]", links.join(", "));
        }
        text.push_str(&line);
        text.push('\n');
    }
    text
}

fn one_line(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

// ---- redacted descriptions (used by the executor; public for later events) -----------------

/// Describes a registry value for a log: the kind, and the number for the
/// numeric kinds, but only the length for strings and binary data.
#[must_use]
pub fn redacted_value(value: &RegistryValue) -> String {
    match value {
        RegistryValue::Dword(v) => format!("REG_DWORD {v}"),
        RegistryValue::Qword(v) => format!("REG_QWORD {v}"),
        RegistryValue::Sz(v) => format!("REG_SZ ({} chars)", v.chars().count()),
        RegistryValue::ExpandSz(v) => format!("REG_EXPAND_SZ ({} chars)", v.chars().count()),
        RegistryValue::MultiSz(v) => format!(
            "REG_MULTI_SZ ({} strings, {} chars)",
            v.len(),
            v.iter().map(|s| s.chars().count()).sum::<usize>()
        ),
        RegistryValue::Binary(v) => format!("REG_BINARY ({} bytes)", v.len()),
    }
}

/// Describes a recorded prior state for a log, with the same redaction.
#[must_use]
pub fn redacted_prior(prior: &PriorState) -> String {
    match prior {
        PriorState::RegistryValue(Some(value)) => redacted_value(value),
        PriorState::RegistryValue(None) | PriorState::RegistryValueInNewKey { .. } => {
            "(not set)".to_owned()
        }
        PriorState::RegistryKeyMissing { .. } => "key missing".to_owned(),
        PriorState::RegistryKeyPresent(contents) => {
            format!("key exists ({} values)", contents.values.len())
        }
        PriorState::ServiceStartType(start_type) => start_type.to_string(),
        PriorState::CommandState(true) => "applied".to_owned(),
        PriorState::CommandState(false) => "not applied".to_owned(),
    }
}

/// What reverting does to a target, in plain words with the same redaction as
/// [`redacted_prior`]: `set back to REG_DWORD 1`, `removed (it did not exist
/// before)`, `set back to disabled`.
#[must_use]
pub fn restore_words(prior: &PriorState) -> String {
    match prior {
        PriorState::RegistryValue(Some(value)) => format!("set back to {}", redacted_value(value)),
        PriorState::RegistryValue(None) => "removed (it did not exist before)".to_owned(),
        PriorState::RegistryValueInNewKey { .. } | PriorState::RegistryKeyMissing { .. } => {
            "removed, with its empty parent keys (it did not exist before)".to_owned()
        }
        PriorState::RegistryKeyPresent(contents) => {
            format!("put back with {} values", contents.values.len())
        }
        PriorState::ServiceStartType(start_type) => format!("set back to {start_type}"),
        PriorState::CommandState(true) => "put back to in effect".to_owned(),
        PriorState::CommandState(false) => "put back to not in effect".to_owned(),
    }
}

/// The class of a port error, without its message (which can carry command
/// output or data).
#[must_use]
pub fn error_class(error: &PortError) -> &'static str {
    match error {
        PortError::NotFound(_) => "not found",
        PortError::AccessDenied(_) => "access denied",
        PortError::Unsupported(_) => "unsupported",
        PortError::InvalidInput(_) => "invalid input",
        PortError::TimedOut(_) => "timed out",
        PortError::Other(_) => "error",
    }
}

/// What an operation touches, named without any data: for a command only the
/// program, never the arguments.
#[must_use]
pub fn subject(operation: &Operation) -> String {
    match operation {
        Operation::SetRegistryValue { target, .. } | Operation::DeleteRegistryValue { target } => {
            match approved_entry_name(target) {
                Some(name) => format!("startup entry {name}"),
                None => format!("registry value {target}"),
            }
        }
        Operation::EnsureRegistryKey { key } | Operation::DeleteRegistryKey { key } => {
            format!("registry key {key}")
        }
        Operation::SetServiceStartType { service, .. } => {
            format!("service {service} start type")
        }
        Operation::RunCommand { apply, .. } => format!("command {}", apply.program),
    }
}

/// `Applied <subject> = <redacted new value>`.
pub(crate) fn applied_summary(operation: &Operation) -> String {
    let subject = subject(operation);
    if let Some(summary) = startup_applied_summary(operation) {
        return summary;
    }
    match operation {
        Operation::SetRegistryValue { value, .. } => {
            format!("Applied {subject} = {}", redacted_value(value))
        }
        Operation::DeleteRegistryValue { .. } => format!("Applied {subject} = (not set)"),
        Operation::EnsureRegistryKey { .. } => format!("Applied {subject} = exists"),
        Operation::DeleteRegistryKey { .. } => format!("Applied {subject} = removed"),
        Operation::SetServiceStartType { start_type, .. } => {
            format!("Applied {subject} = {start_type}")
        }
        Operation::RunCommand { .. } => format!("Applied {subject}"),
    }
}

/// `Reverted <subject>, restored <redacted prior state>` (or `Rolled back`).
pub(crate) fn restored_summary(
    verb: &str,
    operation: &Operation,
    prior: &PriorState,
    journal_note: bool,
) -> String {
    let mut summary = startup_restored_summary(verb, operation, prior).unwrap_or_else(|| {
        format!(
            "{verb} {}, restored {}",
            subject(operation),
            redacted_prior(prior)
        )
    });
    if journal_note {
        summary.push_str(" (the journal entry could not be updated)");
    }
    summary
}

// ---- startup entries: the same change, said in plain words ---------------------------------

/// `Disabled startup entry: Name` or `Enabled startup entry: Name` for a write of an approval
/// record (see [`crate::startup::approval`]); `None` for any other operation.
fn startup_applied_summary(operation: &Operation) -> Option<String> {
    let Operation::SetRegistryValue { target, value } = operation else {
        return None;
    };
    let name = approved_entry_name(target)?;
    match decode_approval(value)? {
        StartupState::Enabled => Some(format!("Enabled startup entry: {name}")),
        StartupState::Disabled => Some(format!("Disabled startup entry: {name}")),
    }
}

/// `Reverted startup entry: Name is enabled again` for the undo of an approval record.
fn startup_restored_summary(
    verb: &str,
    operation: &Operation,
    prior: &PriorState,
) -> Option<String> {
    let (Operation::SetRegistryValue { target, .. } | Operation::DeleteRegistryValue { target }) =
        operation
    else {
        return None;
    };
    let name = approved_entry_name(target)?;
    let previous = prior.registry_value()?;
    let now = match previous.map(decode_approval) {
        // No record means Windows starts the entry.
        None | Some(Some(StartupState::Enabled)) => "is enabled again",
        Some(Some(StartupState::Disabled)) => "is disabled again",
        Some(None) => "has its earlier setting back",
    };
    Some(format!("{verb} startup entry: {name} {now}"))
}

/// [`redacted_prior`] for a journal entry: a startup approval record reads as `enabled` or
/// `disabled` instead of `REG_BINARY (12 bytes)`.
#[must_use]
pub fn redacted_prior_for(operation: &Operation, prior: &PriorState) -> String {
    match startup_prior_words(operation, prior) {
        Some(state) => state.to_owned(),
        None => redacted_prior(prior),
    }
}

/// [`restore_words`] for a journal entry, with the same startup wording.
#[must_use]
pub fn restore_words_for(operation: &Operation, prior: &PriorState) -> String {
    match startup_prior_words(operation, prior) {
        Some("enabled (it had no record)") => {
            "set back to enabled (the record Volt wrote is removed)".to_owned()
        }
        Some(state) => format!("set back to {state}"),
        None => restore_words(prior),
    }
}

/// What a startup approval record held before: `enabled`, `enabled (no record)`, `disabled`.
fn startup_prior_words(operation: &Operation, prior: &PriorState) -> Option<&'static str> {
    let (Operation::SetRegistryValue { target, .. } | Operation::DeleteRegistryValue { target }) =
        operation
    else {
        return None;
    };
    approved_entry_name(target)?;
    let previous = prior.registry_value()?;
    match previous.map(decode_approval) {
        None => Some("enabled (it had no record)"),
        Some(Some(StartupState::Enabled)) => Some("enabled"),
        Some(Some(StartupState::Disabled)) => Some("disabled"),
        Some(None) => None,
    }
}

/// `Failed to <verb> <subject>: <reason>`.
pub(crate) fn failure_summary(verb: &str, operation: &Operation, reason: &str) -> String {
    format!("Failed to {verb} {}: {reason}", subject(operation))
}

/// Why applying failed, in words, without any port message.
#[must_use]
pub fn exec_reason(error: &ExecError) -> String {
    match error {
        ExecError::Detect(e) => format!("could not read the current state ({})", error_class(e)),
        ExecError::Journal(_) => "could not write the undo journal".to_owned(),
        ExecError::Apply { error, .. } => format!("the change failed ({})", error_class(error)),
        ExecError::MarkApplied(_) => {
            "the change was made but the journal entry could not be updated".to_owned()
        }
    }
}

/// Why reverting failed, in words, without any port message.
#[must_use]
pub fn revert_reason(error: &RevertError) -> String {
    match error {
        RevertError::Port(e) => format!(
            "the previous state could not be restored ({})",
            error_class(e)
        ),
        RevertError::Journal(_) => "could not read the undo journal".to_owned(),
        RevertError::EntryNotFound(_) => "the journal entry was not found".to_owned(),
        RevertError::NotRevertible(_) => "the entry cannot be reverted".to_owned(),
        RevertError::MarkReverted(_) => {
            "the state was restored but the journal entry could not be updated".to_owned()
        }
    }
}

#[cfg(test)]
mod tests;
