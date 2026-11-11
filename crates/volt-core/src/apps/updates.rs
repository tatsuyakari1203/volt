//! App updates through winget: listing what it offers and installing one update at a time.
//!
//! This uses the network, and only when the user asks: nothing here runs on its own. winget has
//! no machine-readable form of its upgrade listing, so [`parse_upgrade_list`] reads the table it
//! prints. The parser does not depend on the language of the headers: the table starts below the
//! line of dashes, and each row is read from the right (source, available version, current
//! version, id), so the name may hold spaces and wide characters. A row it cannot trust (a
//! truncated id or version) is counted, never guessed, and output that has no table is reported
//! as such.
//!
//! [`WingetCommand`] is the only thing the [`Winget`] port can be asked to run: a fixed argument
//! list, with the package id and source validated first ([`PackageId`], [`PackageSource`]). No
//! argument ever accepts an agreement: if winget needs the user to accept source terms, the
//! manager says so ([`UpdateProblem::Agreements`]) and the user does it in a terminal.
//!
//! Installing runs winget's own upgrade for one id and reads its exit code. Windows shows its
//! administrator prompt from winget's installer when one is needed; Volt's helper is not
//! involved. An update cannot be undone by Volt, so it is an activity record, not an undo entry.
//! The queue (one after another, stop after the running one) is the page's: each call to
//! [`UpdateManager::upgrade`] runs one package and returns.

use std::sync::{Arc, Mutex, TryLockError};

use thiserror::Error;

use crate::activity::{ActivityLog, ActivityOutcome, NewActivity, error_class};
use crate::error::PortError;
use crate::journal::Source;
use crate::phrase::{Phrase, class_phrase};
use crate::time::Clock;

/// The activity label of updating an app.
pub const UPDATE_LABEL: &str = "apps.update";

/// What winget printed and how it ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WingetRun {
    /// The exit code of winget.
    pub exit_code: i32,
    /// Its output (standard output and error together), capped by the adapter.
    pub output: String,
}

/// A winget package id that is safe to pass as one argument: letters, digits, `.`, `_`, `+` and
/// `-`, starting with a letter or digit.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PackageId(String);

impl PackageId {
    /// The id, or `None` when it is empty, too long, starts with `-` or holds anything else.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let mut chars = text.chars();
        let first = chars.next()?;
        let valid = first.is_ascii_alphanumeric()
            && text.len() <= 128
            && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-'));
        valid.then(|| Self(text.to_owned()))
    }

    /// The id as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The name of a winget source (`winget`, `msstore`): lower-case letters and digits.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PackageSource(String);

impl PackageSource {
    /// The source name, or `None` when it is not a plain name.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let valid = !text.is_empty()
            && text.len() <= 32
            && text.starts_with(|c: char| c.is_ascii_lowercase())
            && text
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
        valid.then(|| Self(text.to_owned()))
    }

    /// The source as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// What the port may run. Each form has a fixed argument list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WingetCommand {
    /// List the packages with an available upgrade.
    List,
    /// Upgrade one package by exact id.
    Upgrade {
        /// The package.
        id: PackageId,
        /// The source it came from, when the listing said.
        source: Option<PackageSource>,
    },
}

impl WingetCommand {
    /// The arguments, never joined into a command line and never run through a shell. None of
    /// them accepts an agreement.
    #[must_use]
    pub fn args(&self) -> Vec<String> {
        let mut args = vec!["upgrade".to_owned()];
        if let Self::Upgrade { id, source } = self {
            args.extend([
                "--id".to_owned(),
                id.as_str().to_owned(),
                "--exact".to_owned(),
            ]);
            if let Some(source) = source {
                args.extend(["--source".to_owned(), source.as_str().to_owned()]);
            }
        }
        args.push("--disable-interactivity".to_owned());
        args
    }
}

/// Port: running winget.
pub trait Winget: Send + Sync {
    /// Runs `command` and waits for it. `NotFound` means winget is not there; any other error
    /// means it could not be run.
    fn run(&self, command: &WingetCommand) -> Result<WingetRun, PortError>;
}

/// One update winget offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppUpdate {
    /// The winget package id. Pass it to [`UpdateManager::upgrade`].
    pub id: String,
    /// The name shown to the user.
    pub name: String,
    /// The installed version.
    pub current: String,
    /// The version winget offers.
    pub available: String,
    /// The winget source, when the table had that column.
    pub source: Option<String>,
}

/// The updates found in winget's output.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ParsedUpdates {
    /// The updates, in the order winget printed them.
    pub updates: Vec<AppUpdate>,
    /// Rows that were left out because their id or version was cut short or unreadable.
    pub skipped: u32,
}

/// Why the output holds no table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoTable {
    /// There is no header and line of dashes.
    Missing,
    /// There is a table, but not with the four or five columns winget prints.
    Columns,
}

/// Removes terminal escape sequences and keeps what a progress line ends with (`\r` rewinds).
fn clean_lines(text: &str) -> Vec<String> {
    text.split('\n')
        .map(|line| {
            let line = line.trim_end_matches(['\r', ' ', '\t']);
            let last = line.rsplit('\r').next().unwrap_or(line);
            let mut out = String::with_capacity(last.len());
            let mut chars = last.chars().peekable();
            while let Some(c) = chars.next() {
                if c == '\u{1b}' {
                    if chars.peek() == Some(&'[') {
                        chars.next();
                        for next in chars.by_ref() {
                            if next.is_ascii_alphabetic() {
                                break;
                            }
                        }
                    }
                } else {
                    out.push(c);
                }
            }
            out.trim_end().to_owned()
        })
        .collect()
}

fn is_rule(line: &str) -> bool {
    let line = line.trim();
    line.len() >= 8 && line.chars().all(|c| c == '-')
}

fn cut_short(token: &str) -> bool {
    token.contains('\u{2026}') || token.contains("...")
}

fn is_version(token: &str) -> bool {
    token.chars().any(|c| c.is_ascii_digit()) && !token.chars().any(char::is_whitespace)
}

/// Splits a row into tokens, joining a lone `<`, `>` or `~` to the version after it.
fn tokens(row: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut join = false;
    for token in row.split_whitespace() {
        if matches!(token, "<" | ">" | "~") {
            out.push(token.to_owned());
            join = true;
        } else if join {
            if let Some(last) = out.last_mut() {
                last.push_str(token);
            }
            join = false;
        } else {
            out.push(token.to_owned());
        }
    }
    out
}

/// Reads winget's upgrade table. See the module documentation for what it relies on.
///
/// # Errors
///
/// [`NoTable`] when the text holds no table, so the caller can tell "no updates" from output it
/// does not understand.
pub fn parse_upgrade_list(text: &str) -> Result<ParsedUpdates, NoTable> {
    let lines = clean_lines(text);
    let rule = (1..lines.len())
        .find(|&i| is_rule(&lines[i]) && !lines[i - 1].trim().is_empty())
        .ok_or(NoTable::Missing)?;
    let columns = lines[rule - 1]
        .split("  ")
        .filter(|part| !part.trim().is_empty())
        .count();
    if !(4..=5).contains(&columns) {
        return Err(NoTable::Columns);
    }
    let with_source = columns == 5;
    let tail = if with_source { 4 } else { 3 };
    let mut found = ParsedUpdates::default();
    for row in &lines[rule + 1..] {
        if row.trim().is_empty() {
            break;
        }
        let parts = tokens(row);
        if parts.len() <= tail {
            break;
        }
        let at = parts.len() - tail;
        let (id, current, available) = (&parts[at], &parts[at + 1], &parts[at + 2]);
        let source = with_source.then(|| &parts[at + 3]);
        let unreadable = [Some(id), Some(current), Some(available), source]
            .into_iter()
            .flatten()
            .any(|token| cut_short(token));
        if unreadable {
            found.skipped += 1;
            continue;
        }
        let source_ok = source.is_none_or(|s| {
            s.starts_with(|c: char| c.is_ascii_alphabetic())
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        });
        if PackageId::parse(id).is_none()
            || !is_version(current)
            || !is_version(available)
            || !source_ok
        {
            // Not a row of the table: the footer or another message starts here.
            break;
        }
        if found.updates.iter().any(|update| update.id == *id) {
            continue;
        }
        found.updates.push(AppUpdate {
            id: id.clone(),
            name: parts[..at].join(" "),
            current: current.clone(),
            available: available.clone(),
            source: source.cloned(),
        });
    }
    Ok(found)
}

/// Why no list could be made. Each has a sentence and a suggested fix.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum UpdateProblem {
    /// winget is not installed (or its alias is off).
    Missing,
    /// winget could not be started. Carries the class of the failure.
    Broken(&'static str),
    /// winget wants the user to accept its source terms first.
    Agreements,
    /// winget printed something that is not a table Volt can read.
    Unrecognised,
    /// winget stopped with this exit code and printed no table.
    Failed(i32),
    /// An update is running.
    Busy,
}

impl UpdateProblem {
    /// What went wrong, as a sentence.
    #[must_use]
    pub fn phrase(&self) -> Phrase {
        match self {
            Self::Missing => Phrase::new("updates.problem.missing"),
            Self::Broken(class) => {
                Phrase::new("updates.problem.broken").phrase("class", class_phrase(class))
            }
            Self::Agreements => Phrase::new("updates.problem.agreements"),
            Self::Unrecognised => Phrase::new("updates.problem.unrecognised"),
            Self::Failed(code) => {
                Phrase::new("updates.problem.failed").text("code", code_text(*code))
            }
            Self::Busy => Phrase::new("updates.problem.busy"),
        }
    }

    /// What the user can do about it. Volt never does it for them.
    #[must_use]
    pub fn fix(&self) -> Phrase {
        Phrase::new(match self {
            Self::Missing => "updates.fix.missing",
            Self::Broken(_) => "updates.fix.broken",
            Self::Agreements => "updates.fix.agreements",
            Self::Unrecognised => "updates.fix.unrecognised",
            Self::Failed(_) => "updates.fix.failed",
            Self::Busy => "updates.fix.busy",
        })
    }
}

/// An exit code as winget documents it: a small number in decimal, an HRESULT in hex.
fn code_text(code: i32) -> String {
    if (0..=0xFFFF).contains(&code) {
        code.to_string()
    } else {
        format!("0x{:08x}", code.cast_unsigned())
    }
}

/// winget's "no applicable update" exit code.
const NOTHING_TO_UPDATE: i32 = 0x8A15_002B_u32.cast_signed();

fn mentions_agreements(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("source agreement") || (lower.contains("[y]") && lower.contains("[n]"))
}

fn meaningful_lines(text: &str) -> usize {
    clean_lines(text)
        .iter()
        .filter(|line| line.chars().any(char::is_alphanumeric))
        .count()
}

/// The updates winget offers, as one reading.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UpdateListing {
    /// The updates.
    pub updates: Vec<AppUpdate>,
    /// Rows left out because they were cut short.
    pub skipped: u32,
}

/// How updating one app ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum UpgradeOutcome {
    /// winget finished without an error.
    Updated,
    /// winget finished; Windows must restart to complete the update.
    NeedsRestart,
    /// It did not finish. Carries the reason.
    Failed(UpgradeFailure),
}

/// Why an update failed. Never winget's own text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpgradeFailure {
    /// winget could not be started. Carries the class of the failure.
    NotStarted(&'static str),
    /// Volt stopped waiting for winget.
    TimedOut,
    /// winget or the installer stopped with this exit code.
    Code(i32),
}

impl UpgradeFailure {
    /// The reason as a sentence.
    #[must_use]
    pub fn phrase(&self) -> Phrase {
        match self {
            Self::NotStarted(class) => {
                Phrase::new("updates.fail.notStarted").phrase("class", class_phrase(class))
            }
            Self::TimedOut => Phrase::new("updates.fail.timedOut"),
            Self::Code(code) => Phrase::new("updates.fail.code").text("code", code_text(*code)),
        }
    }
}

/// How a winget exit code reads. 3010 and 1641 are the Windows codes for "done, restart needed".
fn outcome_of(code: i32) -> UpgradeOutcome {
    match code {
        0 => UpgradeOutcome::Updated,
        3010 | 1641 => UpgradeOutcome::NeedsRestart,
        other => UpgradeOutcome::Failed(UpgradeFailure::Code(other)),
    }
}

/// What updating one app did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpgradeReport {
    /// The package id.
    pub id: String,
    /// Its name.
    pub name: String,
    /// The version that was installed before.
    pub from: String,
    /// The version that was offered.
    pub to: String,
    /// How it ended.
    pub outcome: UpgradeOutcome,
    /// Whether the activity record could not be written (the update itself is unaffected).
    pub activity_write_failed: bool,
}

/// Why nothing was started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum UpdateError {
    /// The id is not in the last check (check again).
    #[error("that update is not in the list; check for updates again")]
    NotFound,
    /// Another check or update is running.
    #[error("another update is running; wait for it to finish")]
    Busy,
}

/// Lists and installs winget updates over the [`Winget`] port.
pub struct UpdateManager {
    winget: Arc<dyn Winget>,
    clock: Arc<dyn Clock>,
    activity: Option<Arc<dyn ActivityLog>>,
    running: Mutex<()>,
    checked: Mutex<Vec<AppUpdate>>,
}

impl std::fmt::Debug for UpdateManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UpdateManager").finish_non_exhaustive()
    }
}

impl UpdateManager {
    /// A manager running winget through `winget`.
    #[must_use]
    pub fn new(winget: Arc<dyn Winget>, clock: Arc<dyn Clock>) -> Self {
        Self {
            winget,
            clock,
            activity: None,
            running: Mutex::new(()),
            checked: Mutex::new(Vec::new()),
        }
    }

    /// Writes one activity record per update to `log`.
    #[must_use]
    pub fn with_activity_log(mut self, log: Arc<dyn ActivityLog>) -> Self {
        self.activity = Some(log);
        self
    }

    /// Asks winget what it can update. Uses the network, so it only runs when the user asked.
    /// Changes nothing on the PC.
    ///
    /// # Errors
    ///
    /// An [`UpdateProblem`] when no list could be made.
    pub fn check(&self) -> Result<UpdateListing, UpdateProblem> {
        let _running = match self.running.try_lock() {
            Ok(guard) => guard,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => return Err(UpdateProblem::Busy),
        };
        let run = self
            .winget
            .run(&WingetCommand::List)
            .map_err(|error| match error {
                PortError::NotFound(_) => UpdateProblem::Missing,
                other => UpdateProblem::Broken(error_class(&other)),
            })?;
        let listing = match parse_upgrade_list(&run.output) {
            Ok(parsed) => UpdateListing {
                updates: parsed.updates,
                skipped: parsed.skipped,
            },
            Err(_) if mentions_agreements(&run.output) => return Err(UpdateProblem::Agreements),
            Err(NoTable::Missing)
                if meaningful_lines(&run.output) <= 3
                    && (run.exit_code == 0 || run.exit_code == NOTHING_TO_UPDATE) =>
            {
                UpdateListing::default()
            }
            Err(_) if run.exit_code != 0 => return Err(UpdateProblem::Failed(run.exit_code)),
            Err(_) => return Err(UpdateProblem::Unrecognised),
        };
        self.checked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone_from(&listing.updates);
        Ok(listing)
    }

    /// Updates the package `id` (an id of the last [`UpdateManager::check`]), waits for winget and
    /// records the result. One update runs at a time.
    ///
    /// # Errors
    ///
    /// [`UpdateError`] when nothing was started.
    pub fn upgrade(&self, id: &str, source: Source) -> Result<UpgradeReport, UpdateError> {
        let _running = match self.running.try_lock() {
            Ok(guard) => guard,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => return Err(UpdateError::Busy),
        };
        let update = self
            .checked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .find(|update| update.id == id)
            .cloned()
            .ok_or(UpdateError::NotFound)?;
        let package = PackageId::parse(&update.id).ok_or(UpdateError::NotFound)?;
        let command = WingetCommand::Upgrade {
            id: package,
            source: update.source.as_deref().and_then(PackageSource::parse),
        };
        let outcome = match self.winget.run(&command) {
            Ok(run) => outcome_of(run.exit_code),
            Err(PortError::TimedOut(_)) => UpgradeOutcome::Failed(UpgradeFailure::TimedOut),
            Err(error) => UpgradeOutcome::Failed(UpgradeFailure::NotStarted(error_class(&error))),
        };
        if matches!(
            outcome,
            UpgradeOutcome::Updated | UpgradeOutcome::NeedsRestart
        ) {
            self.checked
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .retain(|other| other.id != update.id);
        }
        let mut report = UpgradeReport {
            id: update.id,
            name: update.name,
            from: update.current,
            to: update.available,
            outcome,
            activity_write_failed: false,
        };
        report.activity_write_failed = !self.record(&report, source);
        Ok(report)
    }

    fn record(&self, report: &UpgradeReport, source: Source) -> bool {
        let Some(log) = &self.activity else {
            return true;
        };
        let named = |code: &str| {
            Phrase::new(code)
                .text("name", report.name.clone())
                .text("version", report.to.clone())
        };
        let (outcome, phrase) = match report.outcome {
            UpgradeOutcome::Updated => (ActivityOutcome::Applied, named("summary.update.updated")),
            UpgradeOutcome::NeedsRestart => {
                (ActivityOutcome::Applied, named("summary.update.restart"))
            }
            UpgradeOutcome::Failed(failure) => (
                ActivityOutcome::Failed,
                named("summary.update.failed").phrase("reason", failure.phrase()),
            ),
        };
        let activity = NewActivity::said(
            self.clock.now(),
            source,
            outcome,
            phrase,
            Some(UPDATE_LABEL.to_owned()),
        );
        match log.record(activity) {
            Ok(_) => true,
            Err(error) => {
                log::warn!(
                    "could not write the activity record for updating an app ({})",
                    error_class(&error)
                );
                false
            }
        }
    }
}
