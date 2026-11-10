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
        PriorState::RegistryValue(None) => "(not set)".to_owned(),
        PriorState::ServiceStartType(start_type) => start_type.to_string(),
        PriorState::CommandState(true) => "applied".to_owned(),
        PriorState::CommandState(false) => "not applied".to_owned(),
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
        PortError::Other(_) => "error",
    }
}

/// What an operation touches, named without any data: for a command only the
/// program, never the arguments.
pub(crate) fn subject(operation: &Operation) -> String {
    match operation {
        Operation::SetRegistryValue { target, .. } | Operation::DeleteRegistryValue { target } => {
            format!("registry value {target}")
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
    match operation {
        Operation::SetRegistryValue { value, .. } => {
            format!("Applied {subject} = {}", redacted_value(value))
        }
        Operation::DeleteRegistryValue { .. } => format!("Applied {subject} = (not set)"),
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
    let mut summary = format!(
        "{verb} {}, restored {}",
        subject(operation),
        redacted_prior(prior)
    );
    if journal_note {
        summary.push_str(" (the journal entry could not be updated)");
    }
    summary
}

/// `Failed to <verb> <subject>: <reason>`.
pub(crate) fn failure_summary(verb: &str, operation: &Operation, reason: &str) -> String {
    format!("Failed to {verb} {}: {reason}", subject(operation))
}

/// Why applying failed, in words, without any port message.
pub(crate) fn exec_reason(error: &ExecError) -> String {
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
pub(crate) fn revert_reason(error: &RevertError) -> String {
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
