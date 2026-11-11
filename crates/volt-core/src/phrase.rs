//! Sentences as data: a code and its parameters, so the UI can write them in any language.
//!
//! The core never writes a sentence for the user in a language other than English, and it never
//! makes the UI take an English sentence apart to find the figures in it. Where the core has data
//! to put in a sentence (a count, a size, a name), it builds a [`Phrase`]: a stable `code` and
//! named [`Arg`]s, which may themselves be phrases. The UI looks the code up in its own message
//! files. English is the fallback: [`Phrase::english`] writes the sentence from the table
//! [`ENGLISH`], and the core keeps that text next to the phrase for records and for a UI that does
//! not know a code (an old build reading a newer record, or the other way round).
//!
//! `scripts/check-i18n.mjs` reads [`ENGLISH`] and fails when the Vietnamese file lacks a code or
//! uses other placeholders than the English text.
//!
//! # Plurals
//!
//! A phrase with an argument named `count` may have two templates, `code.one` and `code.other`;
//! English picks `.one` when the count is 1. A code without them has one template.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

/// A value of a phrase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "camelCase")]
pub enum Arg {
    /// Text that stays as it is in every language (a name, a path, a registry value).
    Text(String),
    /// A whole number.
    Count(u64),
    /// A number of tenths, always written with one decimal: `35` is 3.5, `50` is 5.0. Written with
    /// the decimal mark of the language.
    Tenths(u64),
    /// A size of files in bytes: powers of 1024, one decimal from KB up (`3.4 MB`).
    Size(u64),
    /// A memory or disk size as the Windows shell writes it (`12.4 GB`, `512 MB`).
    Bytes(u64),
    /// Another phrase.
    Phrase(Phrase),
    /// Several phrases written one after another, joined by `sep` (punctuation, such as `, `).
    List {
        /// What goes between two items.
        sep: String,
        /// The items, in order.
        items: Vec<Phrase>,
    },
}

/// A sentence, or a part of one, as a code and parameters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Phrase {
    /// Stable code, such as `summary.applied`. A code is never reused for another meaning.
    pub code: String,
    /// The values, by placeholder name.
    pub args: BTreeMap<String, Arg>,
}

impl Phrase {
    /// A phrase with no parameters.
    #[must_use]
    pub fn new(code: &str) -> Self {
        Self {
            code: code.to_owned(),
            args: BTreeMap::new(),
        }
    }

    fn with(mut self, name: &str, arg: Arg) -> Self {
        self.args.insert(name.to_owned(), arg);
        self
    }

    /// Adds text that is the same in every language.
    #[must_use]
    pub fn text(self, name: &str, value: impl Into<String>) -> Self {
        self.with(name, Arg::Text(value.into()))
    }

    /// Adds a whole number.
    #[must_use]
    pub fn count<T: TryInto<u64>>(self, name: &str, value: T) -> Self {
        self.with(name, Arg::Count(value.try_into().unwrap_or(u64::MAX)))
    }

    /// Adds a number in tenths.
    #[must_use]
    pub fn tenths(self, name: &str, value: u64) -> Self {
        self.with(name, Arg::Tenths(value))
    }

    /// Adds a size of files.
    #[must_use]
    pub fn size(self, name: &str, bytes: u64) -> Self {
        self.with(name, Arg::Size(bytes))
    }

    /// Adds a memory or disk size.
    #[must_use]
    pub fn bytes(self, name: &str, bytes: u64) -> Self {
        self.with(name, Arg::Bytes(bytes))
    }

    /// Adds another phrase.
    #[must_use]
    pub fn phrase(self, name: &str, phrase: Self) -> Self {
        self.with(name, Arg::Phrase(phrase))
    }

    /// Adds phrases written one after another, joined by `sep`.
    #[must_use]
    pub fn list(self, name: &str, sep: &str, items: Vec<Self>) -> Self {
        self.with(
            name,
            Arg::List {
                sep: sep.to_owned(),
                items,
            },
        )
    }

    /// Adds `phrase` when there is one, and nothing visible otherwise (an empty string).
    #[must_use]
    pub fn maybe(self, name: &str, phrase: Option<Self>) -> Self {
        match phrase {
            Some(phrase) => self.phrase(name, phrase),
            None => self.nothing(name),
        }
    }

    /// Adds nothing visible: the placeholder is written as an empty string.
    #[must_use]
    pub fn nothing(self, name: &str) -> Self {
        self.text(name, "")
    }

    /// The sentence in English: the fallback every record keeps.
    #[must_use]
    pub fn english(&self) -> String {
        self.write_with(&english_template, '.', true)
            .unwrap_or_else(|| "?".to_owned())
    }

    /// The sentence written from another language's templates, for the few places where the shell
    /// writes for the user with no window to do it (the tray). `lookup` gives the template of a
    /// code (`count.file.one`, `count.file.other`, `scan.tooNew`); `decimal` is the decimal mark;
    /// `singular` is whether the language has a form for a count of 1 (`.one`). `None` when this
    /// phrase or a part of it has no template, so the caller falls back to [`Phrase::english`].
    #[must_use]
    pub fn write_with(
        &self,
        lookup: &dyn Fn(&str) -> Option<String>,
        decimal: char,
        singular: bool,
    ) -> Option<String> {
        let template = self.template_in(lookup, singular)?;
        let mut out = String::with_capacity(template.len());
        let mut rest = template.as_str();
        while let Some(start) = rest.find('{') {
            out.push_str(&rest[..start]);
            let after = &rest[start + 1..];
            let Some(end) = after.find('}') else {
                out.push_str(&rest[start..]);
                return Some(out);
            };
            if let Some(arg) = self.args.get(&after[..end]) {
                arg.write_in(&mut out, lookup, decimal, singular)?;
            }
            rest = &after[end + 1..];
        }
        out.push_str(rest);
        Some(out)
    }

    fn template_in(
        &self,
        lookup: &dyn Fn(&str) -> Option<String>,
        singular: bool,
    ) -> Option<String> {
        if let Some(Arg::Count(count)) = self.args.get("count") {
            let form = if singular && *count == 1 {
                "one"
            } else {
                "other"
            };
            if let Some(text) = lookup(&format!("{}.{form}", self.code))
                .or_else(|| lookup(&format!("{}.other", self.code)))
            {
                return Some(text);
            }
        }
        lookup(&self.code)
    }
}

fn english_template(code: &str) -> Option<String> {
    ENGLISH
        .iter()
        .find(|(known, _)| *known == code)
        .map(|(_, text)| (*text).to_owned())
}

impl Arg {
    fn write_in(
        &self,
        out: &mut String,
        lookup: &dyn Fn(&str) -> Option<String>,
        decimal: char,
        singular: bool,
    ) -> Option<()> {
        let number = |text: String| {
            if decimal == '.' {
                text
            } else {
                text.replace('.', &decimal.to_string())
            }
        };
        match self {
            Self::Text(text) => out.push_str(text),
            Self::Count(count) => {
                let _ = write!(out, "{count}");
            }
            Self::Tenths(tenths) => {
                out.push_str(&number(format!("{}.{}", tenths / 10, tenths % 10)));
            }
            Self::Size(bytes) => out.push_str(&number(size_text(*bytes))),
            Self::Bytes(bytes) => out.push_str(&number(bytes_text(*bytes))),
            Self::Phrase(phrase) => out.push_str(&phrase.write_with(lookup, decimal, singular)?),
            Self::List { sep, items } => {
                let parts: Option<Vec<String>> = items
                    .iter()
                    .map(|item| item.write_with(lookup, decimal, singular))
                    .collect();
                out.push_str(&parts?.join(sep));
            }
        }
        Some(())
    }
}

const KIB: u64 = 1024;
const MIB: u64 = KIB * KIB;
const GIB: u64 = MIB * KIB;

/// `1.5 MB` style size (powers of 1024, one decimal from KB up).
#[must_use]
pub fn size_text(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes < KIB {
        return format!("{bytes} B");
    }
    #[allow(clippy::cast_precision_loss, reason = "display only")]
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// Bytes as the Windows shell shows them: binary units, written GB and MB.
#[must_use]
pub fn bytes_text(bytes: u64) -> String {
    #[allow(
        clippy::cast_precision_loss,
        reason = "display only: a byte count above 2^53 is beyond any disk, and one decimal is shown"
    )]
    let gib = bytes as f64 / GIB as f64;
    if bytes >= GIB {
        if gib >= 100.0 {
            format!("{gib:.0} GB")
        } else {
            format!("{gib:.1} GB")
        }
    } else {
        format!("{} MB", bytes / MIB)
    }
}

/// The class of an error, as a phrase: `class.notFound`, `class.accessDenied` and so on. A class
/// text this does not know is the generic `class.error`.
#[must_use]
pub fn class_phrase(class: &str) -> Phrase {
    Phrase::new(match class {
        "not found" => "class.notFound",
        "access denied" => "class.accessDenied",
        "unsupported" => "class.unsupported",
        "invalid input" => "class.invalidInput",
        "timed out" => "class.timedOut",
        _ => "class.error",
    })
}

/// Every code the core writes, with its English text. One entry per line of the form
/// `("code", "text")`: `scripts/check-i18n.mjs` reads this table.
#[rustfmt::skip]
pub const ENGLISH: &[(&str, &str)] = &[
    // Why an operation failed, and error classes.
    ("class.notFound", "not found"),
    ("class.accessDenied", "access denied"),
    ("class.unsupported", "unsupported"),
    ("class.invalidInput", "invalid input"),
    ("class.timedOut", "timed out"),
    ("class.error", "error"),
    ("reason.detect", "could not read the current state ({class})"),
    ("reason.journalWrite", "could not write the undo journal"),
    ("reason.applyFailed", "the change failed ({class})"),
    ("reason.markApplied", "the change was made but the journal entry could not be updated"),
    ("reason.restoreFailed", "the previous state could not be restored ({class})"),
    ("reason.journalRead", "could not read the undo journal"),
    ("reason.entryNotFound", "the journal entry was not found"),
    ("reason.notRevertible", "the entry cannot be reverted"),
    ("reason.markReverted", "the state was restored but the journal entry could not be updated"),
    // What an operation touches.
    ("subject.startupEntry", "startup entry {name}"),
    ("subject.registryValue", "registry value {target}"),
    ("subject.registryKey", "registry key {key}"),
    ("subject.service", "service {service} start type"),
    ("subject.command", "command {program}"),
    ("subject.process", "settings of {name}"),
    // Values and prior states, redacted.
    ("value.dword", "REG_DWORD {value}"),
    ("value.qword", "REG_QWORD {value}"),
    ("value.sz", "REG_SZ ({count} chars)"),
    ("value.expandSz", "REG_EXPAND_SZ ({count} chars)"),
    ("value.multiSz", "REG_MULTI_SZ ({strings} strings, {chars} chars)"),
    ("value.binary", "REG_BINARY ({count} bytes)"),
    ("value.exists", "exists"),
    ("value.removed", "removed"),
    ("start.boot", "Boot"),
    ("start.system", "System"),
    ("start.automatic", "Automatic"),
    ("start.automaticDelayed", "Automatic (delayed)"),
    ("start.manual", "Manual"),
    ("start.disabled", "Disabled"),
    ("prior.notSet", "(not set)"),
    ("prior.keyMissing", "key missing"),
    ("prior.keyExists", "key exists ({count} values)"),
    ("prior.applied", "applied"),
    ("prior.notApplied", "not applied"),
    ("prior.processes.one", "the settings of {count} process"),
    ("prior.processes.other", "the settings of {count} processes"),
    ("startup.enabled", "enabled"),
    ("startup.disabled", "disabled"),
    ("startup.noRecord", "enabled (it had no record)"),
    ("startup.enabledAgain", "is enabled again"),
    ("startup.disabledAgain", "is disabled again"),
    ("startup.earlierBack", "has its earlier setting back"),
    // What undoing does.
    ("restore.setBack", "set back to {to}"),
    ("restore.removed", "removed (it did not exist before)"),
    ("restore.removedKey", "removed, with its empty parent keys (it did not exist before)"),
    ("restore.putBack", "put back with {count} values"),
    ("restore.inEffect", "put back to in effect"),
    ("restore.notInEffect", "put back to not in effect"),
    ("restore.processes.one", "put back the settings of {count} process"),
    ("restore.processes.other", "put back the settings of {count} processes"),
    ("restore.startupNoRecord", "set back to enabled (the record Volt wrote is removed)"),
    // Activity summaries.
    ("verb.reverted", "Reverted"),
    ("verb.rolledBack", "Rolled back"),
    ("verb.apply", "apply"),
    ("verb.revert", "revert"),
    ("verb.rollBack", "roll back"),
    ("summary.applied", "Applied {subject} = {value}"),
    ("summary.appliedPlain", "Applied {subject}"),
    ("summary.startupEnabled", "Enabled startup entry: {name}"),
    ("summary.startupDisabled", "Disabled startup entry: {name}"),
    ("summary.restored", "{verb} {subject}, restored {prior}{note}"),
    ("summary.startupRestored", "{verb} startup entry: {name} {state}{note}"),
    ("summary.journalNote", " (the journal entry could not be updated)"),
    ("summary.processTuned", "Changed {name}: {change}"),
    ("summary.processesEnded", "{verb} {subject}: every process had already ended, so there was nothing to restore{note}"),
    ("summary.ruleFailed", "The saved rule for {name} could not be applied ({reason})"),
    ("summary.ruleApplied", "The saved rule for {name} changed {processes}; new processes of the app follow it without further entries (remove the rule to stop)"),
    ("summary.ruleAppliedPartly", "The saved rule for {name} changed {processes} and left {left} alone because Windows would not let Volt change them; new processes of the app follow it without further entries (remove the rule to stop)"),
    ("tune.list", "{items}"),
    ("tune.priority", "priority {class}"),
    ("tune.affinity.one", "{count} processor core"),
    ("tune.affinity.other", "{count} processor cores"),
    ("tune.efficiency.on", "Efficiency mode on"),
    ("tune.efficiency.off", "Efficiency mode off"),
    ("tune.efficiency.default", "Efficiency mode as Windows decides"),
    ("priority.idle", "low"),
    ("priority.belowNormal", "below normal"),
    ("priority.normal", "normal"),
    ("priority.aboveNormal", "above normal"),
    ("priority.high", "high"),
    ("summary.failed", "Failed to {verb} {subject}: {reason}"),
    ("summary.planFailed", "Failed to apply a plan: {reason}"),
    ("summary.interruptedNever", "Interrupted apply of {subject}: the change never happened"),
    ("summary.interruptedUnknown", "Interrupted apply of {subject}: the resulting state is unknown"),
    ("summary.confirmed", "Confirmed after an interruption: {applied}"),
    // Counts.
    ("count.file.one", "{count} file"),
    ("count.file.other", "{count} files"),
    ("count.emptyFolder.one", "{count} empty folder"),
    ("count.emptyFolder.other", "{count} empty folders"),
    ("count.process.one", "{count} process"),
    ("count.process.other", "{count} processes"),
    ("count.item.one", "{count} item"),
    ("count.item.other", "{count} items"),
    ("join.and", "{first} and {second}"),
    // Cleaning.
    ("summary.cleanRule", "Cleaned rule {rule}: moved {moved} to the Recycle Bin ({size}){left}; space is freed only when the Recycle Bin is emptied"),
    ("summary.leftItems", ", left {items} in place"),
    ("summary.binEmptied", "Emptied the Recycle Bin: {items} ({size}) permanently deleted. This cannot be undone."),
    ("summary.binFailed", "Could not empty the Recycle Bin ({class}). Items may already have been deleted permanently; that cannot be undone."),
    ("summary.largeMoved.one", "Moved {count} large file to the Recycle Bin ({size}){left}; space is freed only when the Recycle Bin is emptied"),
    ("summary.largeMoved.other", "Moved {count} large files to the Recycle Bin ({size}){left}; space is freed only when the Recycle Bin is emptied"),
    ("summary.largeLeft.one", ", left {count} file in place"),
    ("summary.largeLeft.other", ", left {count} files in place"),
    // Why an item was left in place.
    ("skip.ruleUnknown", "the rule is not known"),
    ("skip.rootChanged", "the rule's root changed or is no longer usable"),
    ("skip.outsideRoot", "not inside the rule's root"),
    ("skip.notAllowed", "the rule does not allow this path"),
    ("skip.gone", "already gone"),
    ("skip.changed", "changed since the scan"),
    ("skip.link", "is, or is behind, a link or other reparse point"),
    ("skip.notEmpty", "the folder is not empty"),
    ("skip.locked", "locked, in use or access denied"),
    ("skip.refused", "refused by the file system adapter"),
    ("skip.failed", "could not be moved ({class})"),
    // Scan notes.
    ("scan.skipped", "{parts}"),
    ("scan.tooNew", "{count} too new"),
    ("scan.ageUnknown", "{count} with an unknown age"),
    ("scan.accessDenied", "{count} denied by Windows"),
    ("scan.unreadable", "{count} unreadable"),
    ("scan.links", "{count} links left alone"),
    ("scan.deep", "{count} deep folders not entered"),
    ("scan.oddNames", "{count} with unusual names"),
    ("scan.locked.one", "{names} is running: files it keeps open are skipped. Close it first to clean everything."),
    ("scan.locked.other", "{names} are running: files it keeps open are skipped. Close it first to clean everything."),
    ("scan.problem.root", "Volt could not use this location, so it was not scanned."),
    ("scan.problem.output", "This folder is an output folder, which is never cleaned."),
    ("scan.problem.link", "This location is a link, so it was not scanned."),
    ("scan.problem.notFolder", "This location is not a folder, so it was not scanned."),
    ("scan.problem.denied", "Windows denied access to this location."),
    ("scan.problem.unreadable", "This location could not be read ({class})."),
    ("scan.problem.minAge", "The minimum age setting could not be used, so this rule was not scanned."),
    ("scan.problem.notInstall", "This folder does not look like the tool's install ({names} not found), so it was not scanned."),
    ("scan.problem.generic", "This rule could not be scanned."),
    ("folder.refused.notInstall", "This folder does not look like the tool's install: {names} not found in it."),
    ("folder.refused.output", "This is an output folder, which is never cleaned."),
    ("folder.refused.broad", "This folder is too broad or a protected folder."),
    ("folder.refused.notFolder", "This is not an existing, plain folder."),
    // Memory cleaning.
    ("area.workingSets", "working sets"),
    ("area.standbyList", "standby list"),
    ("area.standbyListLowPriority", "low-priority standby list"),
    ("area.modifiedList", "modified list"),
    ("area.fileCache", "system file cache"),
    ("summary.memory", "{parts}"),
    ("summary.memory.head", "Cleaned memory (cannot be undone)"),
    ("summary.memory.headDone", "Cleaned memory (cannot be undone): {areas}"),
    ("summary.memory.head.admin", "Cleaned memory as administrator (cannot be undone)"),
    ("summary.memory.headDone.admin", "Cleaned memory as administrator (cannot be undone): {areas}"),
    ("summary.memory.trimmed", "{area} ({count} processes)"),
    ("summary.memory.trimmedExcluded", "{area} ({count} processes, {excluded} left alone on the exclusion list)"),
    ("summary.memory.areaFailed", "{area} ({class})"),
    ("summary.memory.couldNot", "could not clean {areas}"),
    ("summary.memory.free", "free memory {before} to {after}, standby {standbyBefore} to {standbyAfter}"),
    // Ending an app.
    ("summary.process", "Ended {name} (cannot be undone): {ended} of {total} processes{admin}{gone}{failed}"),
    ("summary.process.admin", "; {count} need administrator rights"),
    ("summary.process.gone", "; {count} had already exited"),
    ("summary.process.failed", "; {count} could not be ended"),
    // Removing an installed app.
    ("summary.uninstall.removed", "Uninstalled {name} (cannot be undone by Volt)"),
    ("summary.uninstall.stillInstalled", "Ran the uninstaller for {name}; it is still installed"),
    ("summary.uninstall.unknown", "Ran the uninstaller for {name}; Volt could not check whether it was removed (cannot be undone by Volt)"),
    ("summary.uninstall.failed", "Could not uninstall {name}: {reason}"),
    // Health reasons.
    ("health.disk", "{free} of {total} is free on {mount} ({share}%). Windows needs room to install updates and to move memory to disk."),
    ("health.memory", "{percent}% of your memory is in use. When it runs out Windows slows down and may close programs."),
    ("health.restart", "Windows has updates or installs that finish only after a restart."),
    ("health.startup", "{count} startup entries are enabled. Each one starts a program when you sign in, which slows it down."),
    // Why a line on the Undo page cannot be undone.
    ("undo.noRecord", "Volt saved no earlier value for this action, so it cannot restore it."),
    ("undo.recordGone", "Its undo record is no longer in the journal, so Volt cannot restore it."),
    ("undo.unreadable", "The undo journal could not be read just now. Reload the list to try again."),
    ("undo.unconfirmed", "Volt has not confirmed that this change happened."),
    ("undo.accepted", "Something else changed this setting afterwards and you chose to keep that, so Volt will not restore it."),
    // A configuration file, as the import preview describes the settings in it.
    ("transfer.boost", "Boost runs {areas}. Move temporary files to the Recycle Bin: {bin}."),
    ("transfer.list", "{items}"),
    ("transfer.on", "on"),
    ("transfer.off", "off"),
    ("transfer.nothing", "nothing"),
    ("transfer.exclusions.one", "Adds {count} program to the list that memory trimming leaves alone: {names}"),
    ("transfer.exclusions.other", "Adds {count} programs to the list that memory trimming leaves alone: {names}"),
    // What a tweak will change, as the preview shows it.
    ("preview.text", "{text}"),
    ("preview.command", "command {command}"),
    ("preview.keyExists", "key exists"),
    ("preview.keyRemoved", "key removed"),
    ("preview.keyEmpty", "key exists, empty"),
    ("preview.keyValues.one", "key exists, {count} value"),
    ("preview.keyValues.other", "key exists, {count} values"),
    // Errors the shell reports to the UI.
    ("err.cursor", "`{text}` is not an activity cursor"),
    ("err.badId", "`{text}` is not a valid id"),
    ("err.noEntry", "there is no journal entry {id}"),
    ("err.backgroundTask", "the background task failed: {detail}"),
    ("err.locationUnusable", "the chosen location cannot be used: {detail}"),
    ("err.scanRunning", "a scan is already running"),
    ("err.scanBusy", "a scan is running; wait for it to finish or cancel it"),
    ("err.scanStale", "that scan is out of date; scan again"),
    ("err.scanFailed", "the scan could not run"),
    ("err.cleanerUnavailable", "the cleaner is not available in this build"),
    ("err.binUnavailable", "the Recycle Bin is not available in this build"),
    ("err.largeFilesUnavailable", "the large files finder is not available in this build"),
    ("err.memoryUnavailable", "memory cleaning is not available in this build"),
    ("err.noRule", "no rule is chosen"),
    ("err.noItem", "choose at least one item to clean"),
    ("err.unknownRule", "there is no cleaning rule \"{id}\""),
    ("err.ruleNotInScan", "\"{id}\" is not part of this scan"),
    ("err.ruleNeedsAdmin", "\"{name}\" needs administrator rights."),
    ("err.itemsNotChosen", "\"{id}\" has picked items but is not chosen"),
    ("err.notPickable", "\"{id}\" is not picked by item"),
    ("err.noSuchItem", "\"{id}\" has no item {index}"),
    ("err.folder.rule", "this rule does not work in a folder you add"),
    ("err.folder.noSettings", "the choice cannot be saved in this build"),
    ("err.folder.settings", "the choice could not be saved ({class})"),
    ("err.folder.unusable", "this folder cannot be used"),
    ("err.noFile", "no file is chosen"),
    ("err.fileNotInScan", "a chosen file is not part of this scan"),
    ("err.explorer", "Explorer could not be started: {detail}"),
    ("err.openFolderWindows", "opening a folder is only available on Windows"),
    ("err.noMemoryOperation", "choose at least one memory operation"),
    ("err.trimNeedsNoAdmin", "trimming working sets does not need administrator rights: run it without them so the exclusion list applies"),
    ("err.coreRange", "core {core} is out of range"),
    ("err.autostart", "start with Windows: {detail}"),
    ("err.helper.unavailable", "Administrator features are not available."),
    ("err.helper.untrusted", "Volt could not verify the administrator helper, so it sent nothing."),
    ("err.helper.mismatch", "The administrator helper does not match this version of Volt, so nothing was sent. Reinstall Volt to update it."),
    ("err.helper.rateLimited", "The administrator helper asks for a few seconds between purges. Try again in a moment."),
    ("err.helper.refused", "The administrator helper refused the request."),
    ("err.helper.disconnected", "The administrator session ended before it answered. Memory may or may not have been cleaned: read it again and run it once more if needed."),
    ("err.helper.unusable", "The administrator helper could not be used."),
    ("err.startup.invalidId", "`{id}` is not a startup entry id"),
    ("err.startup.notFound", "the startup entry `{id}` was not found"),
    ("err.startup.needsAdmin", "Changing \"{name}\" needs administrator rights."),
    ("err.startup.read", "Volt could not read the startup entry ({class})."),
    ("err.startup.change", "The startup entry could not be changed: {reason}."),
    ("err.tweak.unknown", "unknown tweak `{tweak}`"),
    ("err.tweak.needsAdmin", "tweak `{tweak}` needs administrator rights"),
    ("err.tweak.detect", "tweak `{tweak}`: cannot read the current state ({class})"),
    ("err.tweak.journal", "tweak `{tweak}`: cannot read the undo journal ({class})"),
    ("err.tweak.failed", "tweak `{tweak}`: {reason}"),
    ("err.tweak.nothingToRevert", "tweak `{tweak}`: the undo journal has no applied change to revert"),
    ("err.tweak.nothingToReset", "tweak `{tweak}` cannot be reset to the Windows default"),
    ("err.tweak.appliedByVolt", "tweak `{tweak}` was applied by Volt: undo it instead of resetting it"),
    ("err.config.notVolt", "this is not a Volt configuration file"),
    ("err.config.nothingPending", "no configuration is waiting to be imported"),
    ("err.tweak.notDrifted", "tweak `{tweak}` has not changed since Volt applied it"),
    ("err.uninstall.notFound", "that app is not installed any more"),
    ("err.uninstall.notUninstallable", "this app cannot be removed from Volt"),
    ("err.uninstall.busy", "another app is being removed; wait for it to finish"),
    ("err.uninstall.hidden", "this entry is hidden by default; show system components first"),
    ("err.large.notAbsolute", "the folder is not an absolute path"),
    ("err.large.notFound", "the folder does not exist"),
    ("err.large.notDirectory", "the location is not a plain folder"),
    ("err.large.systemFolder", "system folders are not scanned"),
    ("err.large.systemUnavailable", "the system folders could not be determined ({class})"),
    ("err.large.unreadable", "the folder could not be read ({class})"),
    ("err.large.cancelled", "the scan was cancelled"),
    ("err.adminUnavailable", "administrator actions are not available in this build"),
    ("err.processesUnavailable", "the process list is not available in this build"),
    ("err.tuningUnavailable", "changing process settings is not available in this build"),
    ("err.appsUnavailable", "the installed apps list is not available in this build"),
    ("err.startupUnavailable", "the startup apps manager is not available in this build"),
    ("err.protection.windows", "needed by Windows"),
    ("err.protection.volt", "Volt itself"),
    ("err.process.protected", "{name} cannot be ended: it is {protection}"),
    ("err.process.notRunning", "{name} is not running"),
    ("err.process.list", "the process list could not be read ({class})"),
    ("err.tune.protected", "{name} cannot be changed: it is {protection}"),
    ("err.tune.nothing", "nothing to change was chosen"),
    ("err.tune.emptyAffinity", "at least one core must stay selected"),
    ("err.tune.unknownCore", "a core was chosen that these processes cannot use"),
    ("err.tune.tooMany", "at most {count} rules can be saved"),
    ("err.tune.rules", "the saved rules could not be used ({class})"),
    ("err.tune.read", "the settings could not be read ({class})"),
    ("err.tune.exec", "the change could not be made: {reason}"),
    // Why a tweak did not apply or revert completely.
    ("err.detail", "{parts}"),
    ("err.detail.notRestored.one", "{count} change could not be put back automatically"),
    ("err.detail.notRestored.other", "{count} changes could not be put back automatically"),
    // The tray: menu, tooltip and the notification of a Boost.
    ("tray.open", "Open"),
    ("tray.boost", "Boost"),
    ("tray.quit", "Quit"),
    ("boost.tooltip.failed", "Volt: Boost could not run"),
    ("boost.tooltip.done", "Volt: Boost done"),
    ("boost.tooltip.memory", "Volt: Boost done, memory available {before} to {after}"),
    ("boost.notice.failed", "Boost could not run."),
    ("boost.notice.done", "Boost done."),
    ("boost.notice.memory", "Memory available {before} to {after}."),
    ("boost.notice.moved", "{files} moved to the Recycle Bin."),
    ("boost.notice.skipped", "{count} skipped: open Volt to see why."),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_fills_placeholders_and_nested_phrases() {
        let phrase = Phrase::new("summary.applied")
            .phrase(
                "subject",
                Phrase::new("subject.registryValue").text("target", "HKCU\\X\\Flag"),
            )
            .phrase("value", Phrase::new("value.dword").text("value", "1"));
        assert_eq!(
            phrase.english(),
            "Applied registry value HKCU\\X\\Flag = REG_DWORD 1"
        );
    }

    #[test]
    fn plural_forms_follow_the_count() {
        let one = Phrase::new("count.file").count("count", 1_u32);
        let many = Phrase::new("count.file").count("count", 3_u32);
        assert_eq!(one.english(), "1 file");
        assert_eq!(many.english(), "3 files");
    }

    #[test]
    fn lists_and_numbers_are_written_plainly() {
        let phrase = Phrase::new("summary.memory.free")
            .size("before", 1024 * 1024)
            .size("after", 3 * 1024 * 1024 / 2)
            .size("standbyBefore", 512)
            .size("standbyAfter", 0);
        assert_eq!(
            phrase.english(),
            "free memory 1.0 MB to 1.5 MB, standby 512 B to 0 B"
        );
        let list = Phrase::new("scan.skipped").list(
            "parts",
            ", ",
            vec![
                Phrase::new("scan.tooNew").count("count", 2_u32),
                Phrase::new("scan.unreadable").count("count", 1_u32),
            ],
        );
        assert_eq!(list.english(), "2 too new, 1 unreadable");
        let share = Phrase::new("health.disk")
            .bytes("free", 5 * GIB / 2)
            .bytes("total", 100 * GIB)
            .text("mount", "C:")
            .tenths("share", 25);
        assert!(
            share
                .english()
                .starts_with("2.5 GB of 100 GB is free on C: (2.5%).")
        );
    }

    #[test]
    fn every_code_has_one_template_or_a_pair_of_plural_forms() {
        for (code, text) in ENGLISH {
            assert!(!text.is_empty(), "{code} is empty");
            let base = code
                .strip_suffix(".one")
                .or_else(|| code.strip_suffix(".other"));
            if let Some(base) = base {
                assert!(
                    ENGLISH.iter().any(|(c, _)| *c == format!("{base}.one"))
                        && ENGLISH.iter().any(|(c, _)| *c == format!("{base}.other")),
                    "{base} needs both plural forms"
                );
            }
        }
        let mut codes: Vec<&str> = ENGLISH.iter().map(|(code, _)| *code).collect();
        codes.sort_unstable();
        let before = codes.len();
        codes.dedup();
        assert_eq!(before, codes.len(), "a code is listed twice");
    }

    #[test]
    fn a_phrase_survives_json() {
        let phrase = Phrase::new("scan.locked")
            .text("names", "chrome")
            .count("count", 1_u32);
        let json = serde_json::to_string(&phrase).expect("serialize");
        let back: Phrase = serde_json::from_str(&json).expect("parse");
        assert_eq!(back, phrase);
    }
}
