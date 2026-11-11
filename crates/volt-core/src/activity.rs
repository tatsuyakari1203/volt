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
use crate::phrase::{Phrase, class_phrase};
use crate::registry::RegistryValue;
use crate::service::ServiceStartType;
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
    /// The summary as a code and parameters, so the UI can write it in its own language.
    /// `summary` stays the English text of the same phrase.
    #[serde(default)]
    pub phrase: Option<Phrase>,
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
    /// The summary as a code and parameters. `None` for a record written before phrases existed:
    /// its `summary` is all there is.
    #[serde(default)]
    pub phrase: Option<Phrase>,
}

impl NewActivity {
    /// An event whose summary is `phrase`; the summary text is its English.
    #[must_use]
    pub fn said(
        timestamp: Timestamp,
        source: Source,
        outcome: ActivityOutcome,
        phrase: Phrase,
        label: Option<String>,
    ) -> Self {
        Self {
            timestamp,
            source,
            outcome,
            summary: phrase.english(),
            batch: None,
            entry: None,
            label,
            phrase: Some(phrase),
        }
    }
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

    /// Reads records oldest first (by id), at most `limit` of them, with an id above `after`
    /// (`None` for the very start). The id of the last record is the cursor for the next call, so
    /// a reader that mirrors the log keeps the cursor and misses nothing. A `limit` of zero
    /// returns nothing.
    fn list_after(
        &self,
        after: Option<ActivityId>,
        limit: usize,
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
//
// Every description is a [`Phrase`]: the UI writes it in the language of the user and the core
// keeps the English text (`.english()`) for records and as the fallback. The functions that
// return a `String` are the English text of the phrase of the same name.

/// Describes a registry value for a log: the kind, and the number for the
/// numeric kinds, but only the length for strings and binary data.
#[must_use]
pub fn value_phrase(value: &RegistryValue) -> Phrase {
    match value {
        RegistryValue::Dword(v) => Phrase::new("value.dword").text("value", v.to_string()),
        RegistryValue::Qword(v) => Phrase::new("value.qword").text("value", v.to_string()),
        RegistryValue::Sz(v) => Phrase::new("value.sz").count("count", v.chars().count()),
        RegistryValue::ExpandSz(v) => {
            Phrase::new("value.expandSz").count("count", v.chars().count())
        }
        RegistryValue::MultiSz(v) => Phrase::new("value.multiSz")
            .count("strings", v.len())
            .count("chars", v.iter().map(|s| s.chars().count()).sum::<usize>()),
        RegistryValue::Binary(v) => Phrase::new("value.binary").count("count", v.len()),
    }
}

/// [`value_phrase`] in English.
#[must_use]
pub fn redacted_value(value: &RegistryValue) -> String {
    value_phrase(value).english()
}

/// A service start type as a phrase.
#[must_use]
pub fn start_type_phrase(start_type: ServiceStartType) -> Phrase {
    Phrase::new(match start_type {
        ServiceStartType::Boot => "start.boot",
        ServiceStartType::System => "start.system",
        ServiceStartType::Automatic => "start.automatic",
        ServiceStartType::AutomaticDelayed => "start.automaticDelayed",
        ServiceStartType::Manual => "start.manual",
        ServiceStartType::Disabled => "start.disabled",
    })
}

/// Describes a recorded prior state for a log, with the same redaction.
#[must_use]
pub fn prior_phrase(prior: &PriorState) -> Phrase {
    match prior {
        PriorState::RegistryValue(Some(value)) => value_phrase(value),
        PriorState::RegistryValue(None) | PriorState::RegistryValueInNewKey { .. } => {
            Phrase::new("prior.notSet")
        }
        PriorState::RegistryKeyMissing { .. } => Phrase::new("prior.keyMissing"),
        PriorState::RegistryKeyPresent(contents) => {
            Phrase::new("prior.keyExists").count("count", contents.values.len())
        }
        PriorState::ServiceStartType(start_type) => start_type_phrase(*start_type),
        PriorState::CommandState(true) => Phrase::new("prior.applied"),
        PriorState::CommandState(false) => Phrase::new("prior.notApplied"),
        PriorState::Processes(settings) => {
            Phrase::new("prior.processes").count("count", settings.iter().flatten().count())
        }
    }
}

/// [`prior_phrase`] in English.
#[must_use]
pub fn redacted_prior(prior: &PriorState) -> String {
    prior_phrase(prior).english()
}

/// What reverting does to a target, in plain words with the same redaction as
/// [`prior_phrase`]: `set back to REG_DWORD 1`, `removed (it did not exist
/// before)`, `set back to disabled`.
#[must_use]
pub fn restore_phrase(prior: &PriorState) -> Phrase {
    let set_back = |to: Phrase| Phrase::new("restore.setBack").phrase("to", to);
    match prior {
        PriorState::RegistryValue(Some(value)) => set_back(value_phrase(value)),
        PriorState::RegistryValue(None) => Phrase::new("restore.removed"),
        PriorState::RegistryValueInNewKey { .. } | PriorState::RegistryKeyMissing { .. } => {
            Phrase::new("restore.removedKey")
        }
        PriorState::RegistryKeyPresent(contents) => {
            Phrase::new("restore.putBack").count("count", contents.values.len())
        }
        PriorState::ServiceStartType(start_type) => set_back(start_type_phrase(*start_type)),
        PriorState::CommandState(true) => Phrase::new("restore.inEffect"),
        PriorState::CommandState(false) => Phrase::new("restore.notInEffect"),
        PriorState::Processes(settings) => {
            Phrase::new("restore.processes").count("count", settings.iter().flatten().count())
        }
    }
}

/// [`restore_phrase`] in English.
#[must_use]
pub fn restore_words(prior: &PriorState) -> String {
    restore_phrase(prior).english()
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

/// [`error_class`] as a phrase.
#[must_use]
pub fn error_class_phrase(error: &PortError) -> Phrase {
    class_phrase(error_class(error))
}

/// What an operation touches, named without any data: for a command only the
/// program, never the arguments.
#[must_use]
pub fn subject_phrase(operation: &Operation) -> Phrase {
    match operation {
        Operation::SetRegistryValue { target, .. } | Operation::DeleteRegistryValue { target } => {
            match approved_entry_name(target) {
                Some(name) => Phrase::new("subject.startupEntry").text("name", name.to_string()),
                None => Phrase::new("subject.registryValue").text("target", target.to_string()),
            }
        }
        Operation::EnsureRegistryKey { key } | Operation::DeleteRegistryKey { key } => {
            Phrase::new("subject.registryKey").text("key", key.to_string())
        }
        Operation::SetServiceStartType { service, .. } => {
            Phrase::new("subject.service").text("service", service.to_string())
        }
        Operation::RunCommand { apply, .. } => {
            Phrase::new("subject.command").text("program", apply.program.to_string())
        }
        Operation::TuneProcesses { name, .. } => {
            Phrase::new("subject.process").text("name", name.clone())
        }
    }
}

/// [`subject_phrase`] in English.
#[must_use]
pub fn subject(operation: &Operation) -> String {
    subject_phrase(operation).english()
}

/// `Applied <subject> = <redacted new value>`.
pub(crate) fn applied_summary(operation: &Operation) -> Phrase {
    if let Some(summary) = startup_applied_summary(operation) {
        return summary;
    }
    let applied = |value: Phrase| {
        Phrase::new("summary.applied")
            .phrase("subject", subject_phrase(operation))
            .phrase("value", value)
    };
    match operation {
        Operation::SetRegistryValue { value, .. } => applied(value_phrase(value)),
        Operation::DeleteRegistryValue { .. } => applied(Phrase::new("prior.notSet")),
        Operation::EnsureRegistryKey { .. } => applied(Phrase::new("value.exists")),
        Operation::DeleteRegistryKey { .. } => applied(Phrase::new("value.removed")),
        Operation::SetServiceStartType { start_type, .. } => {
            applied(start_type_phrase(*start_type))
        }
        Operation::RunCommand { .. } => {
            Phrase::new("summary.appliedPlain").phrase("subject", subject_phrase(operation))
        }
        Operation::TuneProcesses { name, change, .. } => {
            crate::tuning::applied_phrase_of(name, change)
        }
    }
}

/// How an entry was put back: on request, or automatically after a later step failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RestoreVerb {
    /// The user asked for the undo.
    Reverted,
    /// A later step of the same plan failed.
    RolledBack,
}

impl RestoreVerb {
    fn done(self) -> Phrase {
        Phrase::new(match self {
            Self::Reverted => "verb.reverted",
            Self::RolledBack => "verb.rolledBack",
        })
    }

    /// The infinitive, for a failure: `revert`, `roll back`.
    pub(crate) fn attempt(self) -> Phrase {
        Phrase::new(match self {
            Self::Reverted => "verb.revert",
            Self::RolledBack => "verb.rollBack",
        })
    }
}

/// `Reverted <subject>, restored <redacted prior state>` (or `Rolled back`).
pub(crate) fn restored_summary(
    verb: RestoreVerb,
    operation: &Operation,
    prior: &PriorState,
    journal_note: bool,
) -> Phrase {
    let with_note = |phrase: Phrase| {
        if journal_note {
            phrase.phrase("note", Phrase::new("summary.journalNote"))
        } else {
            phrase.nothing("note")
        }
    };
    if let Some((name, state)) = startup_restored_state(operation, prior) {
        return with_note(
            Phrase::new("summary.startupRestored")
                .phrase("verb", verb.done())
                .text("name", name)
                .phrase("state", state),
        );
    }
    with_note(
        Phrase::new("summary.restored")
            .phrase("verb", verb.done())
            .phrase("subject", subject_phrase(operation))
            .phrase("prior", prior_phrase_for(operation, prior)),
    )
}

/// `Reverted <subject>: every process had already ended, so there was nothing to restore`.
pub(crate) fn nothing_to_restore_summary(
    verb: RestoreVerb,
    operation: &Operation,
    journal_note: bool,
) -> Phrase {
    Phrase::new("summary.processesEnded")
        .phrase("verb", verb.done())
        .phrase("subject", subject_phrase(operation))
        .maybe(
            "note",
            journal_note.then(|| Phrase::new("summary.journalNote")),
        )
}

// ---- startup entries: the same change, said in plain words ---------------------------------

/// `Disabled startup entry: Name` or `Enabled startup entry: Name` for a write of an approval
/// record (see [`crate::startup::approval`]); `None` for any other operation.
fn startup_applied_summary(operation: &Operation) -> Option<Phrase> {
    let Operation::SetRegistryValue { target, value } = operation else {
        return None;
    };
    let name = approved_entry_name(target)?;
    let code = match decode_approval(value)? {
        StartupState::Enabled => "summary.startupEnabled",
        StartupState::Disabled => "summary.startupDisabled",
    };
    Some(Phrase::new(code).text("name", name.to_string()))
}

/// The entry and `is enabled again` for the undo of an approval record.
fn startup_restored_state(operation: &Operation, prior: &PriorState) -> Option<(String, Phrase)> {
    let (Operation::SetRegistryValue { target, .. } | Operation::DeleteRegistryValue { target }) =
        operation
    else {
        return None;
    };
    let name = approved_entry_name(target)?;
    let previous = prior.registry_value()?;
    let now = match previous.map(decode_approval) {
        // No record means Windows starts the entry.
        None | Some(Some(StartupState::Enabled)) => "startup.enabledAgain",
        Some(Some(StartupState::Disabled)) => "startup.disabledAgain",
        Some(None) => "startup.earlierBack",
    };
    Some((name.to_string(), Phrase::new(now)))
}

/// [`prior_phrase`] for a journal entry: a startup approval record reads as `enabled` or
/// `disabled` instead of `REG_BINARY (12 bytes)`.
#[must_use]
pub fn prior_phrase_for(operation: &Operation, prior: &PriorState) -> Phrase {
    if let Some(settings) = process_prior(operation, prior) {
        return settings;
    }
    match startup_prior_words(operation, prior) {
        Some(state) => Phrase::new(state),
        None => prior_phrase(prior),
    }
}

/// What processes held before a change, for the fields the change touched.
fn process_prior(operation: &Operation, prior: &PriorState) -> Option<Phrase> {
    let (Operation::TuneProcesses { change, .. }, PriorState::Processes(before)) =
        (operation, prior)
    else {
        return None;
    };
    crate::tuning::prior_phrase_of(change, before)
}

/// [`prior_phrase_for`] in English.
#[must_use]
pub fn redacted_prior_for(operation: &Operation, prior: &PriorState) -> String {
    prior_phrase_for(operation, prior).english()
}

/// [`restore_phrase`] for a journal entry, with the same startup wording.
#[must_use]
pub fn restore_phrase_for(operation: &Operation, prior: &PriorState) -> Phrase {
    if let Some(settings) = process_prior(operation, prior) {
        return Phrase::new("restore.setBack").phrase("to", settings);
    }
    match startup_prior_words(operation, prior) {
        Some("startup.noRecord") => Phrase::new("restore.startupNoRecord"),
        Some(state) => Phrase::new("restore.setBack").phrase("to", Phrase::new(state)),
        None => restore_phrase(prior),
    }
}

/// [`restore_phrase_for`] in English.
#[must_use]
pub fn restore_words_for(operation: &Operation, prior: &PriorState) -> String {
    restore_phrase_for(operation, prior).english()
}

/// What a startup approval record held before: `enabled`, `enabled (no record)`, `disabled`, as
/// the code of the phrase that says it.
fn startup_prior_words(operation: &Operation, prior: &PriorState) -> Option<&'static str> {
    let (Operation::SetRegistryValue { target, .. } | Operation::DeleteRegistryValue { target }) =
        operation
    else {
        return None;
    };
    approved_entry_name(target)?;
    let previous = prior.registry_value()?;
    match previous.map(decode_approval) {
        None => Some("startup.noRecord"),
        Some(Some(StartupState::Enabled)) => Some("startup.enabled"),
        Some(Some(StartupState::Disabled)) => Some("startup.disabled"),
        Some(None) => None,
    }
}

/// `Failed to <verb> <subject>: <reason>`.
pub(crate) fn failure_summary(verb: Phrase, operation: &Operation, reason: Phrase) -> Phrase {
    Phrase::new("summary.failed")
        .phrase("verb", verb)
        .phrase("subject", subject_phrase(operation))
        .phrase("reason", reason)
}

/// Why applying failed, in words, without any port message.
#[must_use]
pub fn exec_phrase(error: &ExecError) -> Phrase {
    match error {
        ExecError::Detect(e) => Phrase::new("reason.detect").phrase("class", error_class_phrase(e)),
        ExecError::Journal(_) => Phrase::new("reason.journalWrite"),
        ExecError::Apply { error, .. } => {
            Phrase::new("reason.applyFailed").phrase("class", error_class_phrase(error))
        }
        ExecError::MarkApplied(_) => Phrase::new("reason.markApplied"),
    }
}

/// [`exec_phrase`] in English.
#[must_use]
pub fn exec_reason(error: &ExecError) -> String {
    exec_phrase(error).english()
}

/// Why reverting failed, in words, without any port message.
#[must_use]
pub fn revert_phrase(error: &RevertError) -> Phrase {
    match error {
        RevertError::Port(e) => {
            Phrase::new("reason.restoreFailed").phrase("class", error_class_phrase(e))
        }
        RevertError::Journal(_) => Phrase::new("reason.journalRead"),
        RevertError::EntryNotFound(_) => Phrase::new("reason.entryNotFound"),
        RevertError::NotRevertible(_) => Phrase::new("reason.notRevertible"),
        RevertError::MarkReverted(_) => Phrase::new("reason.markReverted"),
    }
}

/// [`revert_phrase`] in English.
#[must_use]
pub fn revert_reason(error: &RevertError) -> String {
    revert_phrase(error).english()
}

#[cfg(test)]
mod tests;
