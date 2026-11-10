//! The health score: a number from 0 to 100 that always explains itself.
//!
//! [`assess`] turns a plain [`HealthInputs`] into a [`HealthReport`]: a list of [`Deduction`]s and
//! the checks that could not run. Every deduction carries a stable id, its points, a plain-words
//! reason and a fix from the closed set [`FixAction`]. **The score is never stored: it is
//! computed from the listed deductions as 100 minus their points** ([`HealthReport::score`]), so
//! it cannot change without a reason the user can see. A test proves that over a grid of inputs.
//!
//! # Rules
//!
//! Each rule has named thresholds, each with a one-line justification, next to the code that
//! uses it. The rules are small on purpose: a rule that cannot be explained in one sentence
//! does not belong here (see `docs/architecture.md`).
//!
//! # When it is computed
//!
//! Memory pressure changes second by second, so the score is **computed on demand**, when Home
//! opens or the user asks to check again, and never on every metrics tick: a number that jumps
//! while the user reads it would look like a fault in Volt, and the live cards already show the
//! moving values. [`inspect`] reads the system once and builds the report.
//!
//! # What is not checked
//!
//! An input that could not be read is listed in [`HealthReport::unchecked`] and costs no points.
//! It is never guessed: "we could not look" is not "it is fine" and is reported as such. The
//! startup load comes from the startup manager: the number of enabled startup entries. When the
//! list cannot be read completely it is `None` and the check is listed as unchecked.

mod restart;
#[cfg(test)]
mod tests;

use crate::activity::error_class;
use crate::metrics::{DiskSnapshot, MetricsSource, disk_snapshot, memory_snapshot};
use crate::phrase::Phrase;
use crate::registry::Registry;
use crate::startup::{StartupListing, StartupState};

pub use restart::{RestartMarker, pending_restart_markers};

/// The score of a machine with nothing to deduct.
pub const SCORE_MAX: u8 = 100;

// --- Disk space -------------------------------------------------------------------------------

/// Below this share of free space the system disk is critical: Windows Update, the page file
/// and the hibernation file all fail first when the disk is almost full.
pub const DISK_CRITICAL_FREE_PERCENT: u64 = 5;
/// Below this share the system disk is low: updates and large installs start to fail, and
/// fragmentation grows (a common rule of thumb for NTFS is to keep about 10 percent free).
pub const DISK_LOW_FREE_PERCENT: u64 = 10;
/// Below this share the disk is worth watching, before it becomes a problem.
pub const DISK_WATCH_FREE_PERCENT: u64 = 15;
/// Less free space than this is critical on any disk: a feature update alone needs several GB
/// of working space, however large the disk is.
pub const DISK_CRITICAL_FREE_BYTES: u64 = 5 * GIB;
/// With at least this much free space a disk is never penalised, however large it is: 5 percent
/// of a 4 TB disk is 200 GB, which is plenty of room.
pub const DISK_ROOMY_FREE_BYTES: u64 = 100 * GIB;
/// Points for a critical system disk: the biggest single deduction, because a full system disk
/// breaks updates and slows everything.
pub const DISK_CRITICAL_POINTS: u8 = 30;
/// Points for a low system disk.
pub const DISK_LOW_POINTS: u8 = 20;
/// Points for a system disk worth watching.
pub const DISK_WATCH_POINTS: u8 = 10;

// --- Memory pressure --------------------------------------------------------------------------

/// At this share of memory in use or more, Windows is out of room: it starts to compress memory
/// and write it to the page file, and programs can be closed. "In use" counts only memory that
/// cannot be given back at once (the standby cache is available memory), so it is real pressure.
pub const MEMORY_CRITICAL_USED_PERCENT: f32 = 90.0;
/// At this share or more the machine is under pressure and one more program may tip it over.
pub const MEMORY_HIGH_USED_PERCENT: f32 = 80.0;
/// Points for critical memory pressure.
pub const MEMORY_CRITICAL_POINTS: u8 = 25;
/// Points for high memory pressure.
pub const MEMORY_HIGH_POINTS: u8 = 12;

// --- Pending restart --------------------------------------------------------------------------

/// Points for a restart that Windows is waiting for: updates and installs are not finished, and
/// it costs nothing but a restart to clear, so it stays a small deduction.
pub const RESTART_PENDING_POINTS: u8 = 10;

// --- Startup load -----------------------------------------------------------------------------

/// At this many enabled startup entries or more, sign-in is clearly slowed: each one starts a
/// program while Windows is still starting, and a typical clean PC has well under ten.
pub const STARTUP_MANY_APPS: u32 = 15;
/// At this many enabled startup entries or more, sign-in starts to feel the load.
pub const STARTUP_SOME_APPS: u32 = 8;
/// Points for many enabled startup entries.
pub const STARTUP_MANY_POINTS: u8 = 15;
/// Points for some enabled startup entries.
pub const STARTUP_SOME_POINTS: u8 = 8;

// --- Levels -----------------------------------------------------------------------------------

/// A score at or above this is Good.
pub const GOOD_MIN_SCORE: u8 = 80;
/// A score at or above this (and below [`GOOD_MIN_SCORE`]) is Fair; below it the PC is Poor.
pub const FAIR_MIN_SCORE: u8 = 60;

/// The most points a report can deduct: the largest deduction of each check added up. The score
/// therefore never goes below `SCORE_MAX - MAX_TOTAL_DEDUCTION`.
pub const MAX_TOTAL_DEDUCTION: u8 =
    DISK_CRITICAL_POINTS + MEMORY_CRITICAL_POINTS + RESTART_PENDING_POINTS + STARTUP_MANY_POINTS;

/// Every deduction id the rules can produce, in the order they are listed. Ids are stable: the
/// UI and tests may rely on them, so an id is never reused for a different meaning.
pub const DEDUCTION_IDS: [&str; 8] = [
    "disk.critical",
    "disk.low",
    "disk.watch",
    "memory.critical",
    "memory.high",
    "restart.pending",
    "startup.many",
    "startup.some",
];

const GIB: u64 = 1024 * 1024 * 1024;

/// What the user can do about a deduction. A closed set: the UI maps each one to a screen or a
/// sentence, and a new fix means a new variant that every `match` must handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum FixAction {
    /// Open the Cleaner, to free disk space.
    OpenCleaner,
    /// Open Memory, to see what holds it and release what can be released.
    OpenMemory,
    /// Open Startup, to see and change what starts with Windows.
    OpenStartup,
    /// Restart Windows. Volt never restarts the PC itself: this is advice for the user.
    RestartWindows,
}

/// One input of the score, for saying which could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum HealthCheck {
    /// Free space on the system disk.
    SystemDisk,
    /// Memory pressure.
    Memory,
    /// Whether Windows waits for a restart.
    PendingRestart,
    /// Load of the apps that start with Windows.
    Startup,
}

/// Space on one disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskSpace {
    /// Where the volume is mounted, such as `C:\`.
    pub mount: String,
    /// Capacity in bytes.
    pub total_bytes: u64,
    /// Free space in bytes.
    pub free_bytes: u64,
}

impl From<&DiskSnapshot> for DiskSpace {
    fn from(disk: &DiskSnapshot) -> Self {
        Self {
            mount: disk.mount.clone(),
            total_bytes: disk.total_bytes,
            free_bytes: disk.free_bytes,
        }
    }
}

/// How heavily the apps that start with Windows load the machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StartupLoad {
    /// Startup entries that are enabled, from every source the startup manager reads.
    pub enabled_apps: u32,
}

impl StartupLoad {
    /// The load of a startup listing. `None` when a source could not be read: a partial count
    /// would understate the load, and "could not tell" is not "fine".
    #[must_use]
    pub fn from_listing(listing: &StartupListing) -> Option<Self> {
        if !listing.problems.is_empty() {
            return None;
        }
        let enabled = listing
            .entries
            .iter()
            .filter(|entry| entry.state == StartupState::Enabled)
            .count();
        Some(Self {
            enabled_apps: u32::try_from(enabled).unwrap_or(u32::MAX),
        })
    }
}

/// Everything the rules look at. `None` means "could not be read" (or, for the startup load,
/// "not available yet"), never "fine".
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HealthInputs {
    /// Space on the disk Windows is installed on.
    pub system_disk: Option<DiskSpace>,
    /// Memory in use as a share of the installed memory, 0 to 100.
    pub memory_used_percent: Option<f32>,
    /// Whether Windows waits for a restart.
    pub restart_pending: Option<bool>,
    /// Load of the startup apps. `None` when the startup manager could not read them all.
    pub startup_load: Option<StartupLoad>,
}

/// One reason the score is below 100.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deduction {
    /// Stable id, one of [`DEDUCTION_IDS`].
    pub id: &'static str,
    /// The check that caused it.
    pub check: HealthCheck,
    /// Points taken off the score. Always above zero.
    pub points: u8,
    /// A short name for the problem.
    pub title: &'static str,
    /// One or two sentences, in plain words, saying what was found and why it matters, in
    /// English. It is the text of [`Deduction::reason_phrase`] and the fallback of the UI.
    pub reason: String,
    /// The same reason as a code (`health.disk`, `health.memory`, `health.restart`,
    /// `health.startup`) and the figures it was built from, so the UI writes it in its own
    /// language without taking the English apart.
    pub reason_phrase: Phrase,
    /// What to do about it.
    pub fix: FixAction,
}

/// A rough grade of the score for the headline. The number and the list stay the explanation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum HealthLevel {
    /// [`GOOD_MIN_SCORE`] or more.
    Good,
    /// At least [`FAIR_MIN_SCORE`].
    Fair,
    /// Below [`FAIR_MIN_SCORE`].
    Poor,
    /// No check could run, so there is no honest grade.
    Unknown,
}

/// The result of [`assess`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthReport {
    deductions: Vec<Deduction>,
    unchecked: Vec<HealthCheck>,
    checks_run: u8,
}

impl HealthReport {
    /// The score, 0 to 100: [`SCORE_MAX`] minus the points of [`HealthReport::deductions`], and
    /// nothing else.
    #[must_use]
    pub fn score(&self) -> u8 {
        let deducted: u16 = self.deductions.iter().map(|d| u16::from(d.points)).sum();
        // Bounded by MAX_TOTAL_DEDUCTION, so this never saturates; it is a floor, not a rule.
        SCORE_MAX.saturating_sub(u8::try_from(deducted).unwrap_or(SCORE_MAX))
    }

    /// Why the score is below 100. Empty when nothing was found. In rule order: disk, memory,
    /// restart, startup.
    #[must_use]
    pub fn deductions(&self) -> &[Deduction] {
        &self.deductions
    }

    /// The checks that could not run, so their absence from the list is not good news.
    #[must_use]
    pub fn unchecked(&self) -> &[HealthCheck] {
        &self.unchecked
    }

    /// The grade of the score, or [`HealthLevel::Unknown`] when no check could run.
    #[must_use]
    pub fn level(&self) -> HealthLevel {
        if self.checks_run == 0 {
            return HealthLevel::Unknown;
        }
        match self.score() {
            score if score >= GOOD_MIN_SCORE => HealthLevel::Good,
            score if score >= FAIR_MIN_SCORE => HealthLevel::Fair,
            _ => HealthLevel::Poor,
        }
    }
}

/// Applies every rule to `inputs`. Pure: the same inputs always give the same report.
#[must_use]
pub fn assess(inputs: &HealthInputs) -> HealthReport {
    let mut deductions = Vec::new();
    let mut unchecked = Vec::new();
    let mut checks_run = 0_u8;

    match &inputs.system_disk {
        Some(disk) => {
            checks_run += 1;
            deductions.extend(disk_deduction(disk));
        }
        None => unchecked.push(HealthCheck::SystemDisk),
    }
    match inputs.memory_used_percent.filter(|value| value.is_finite()) {
        Some(used) => {
            checks_run += 1;
            deductions.extend(memory_deduction(used));
        }
        None => unchecked.push(HealthCheck::Memory),
    }
    match inputs.restart_pending {
        Some(pending) => {
            checks_run += 1;
            deductions.extend(pending.then(restart_deduction));
        }
        None => unchecked.push(HealthCheck::PendingRestart),
    }
    match inputs.startup_load {
        Some(load) => {
            checks_run += 1;
            deductions.extend(startup_deduction(load));
        }
        None => unchecked.push(HealthCheck::Startup),
    }

    HealthReport {
        deductions,
        unchecked,
        checks_run,
    }
}

/// How serious the free space of a disk is, from the thresholds above. `None` is fine.
fn disk_tier(disk: &DiskSpace) -> Option<(&'static str, u8, &'static str)> {
    let free = disk.free_bytes.min(disk.total_bytes);
    if free >= DISK_ROOMY_FREE_BYTES {
        return None;
    }
    // `free / total < percent / 100`, compared as integers so no rounding decides a boundary.
    let below =
        |percent: u64| u128::from(free) * 100 < u128::from(disk.total_bytes) * u128::from(percent);
    if free < DISK_CRITICAL_FREE_BYTES || below(DISK_CRITICAL_FREE_PERCENT) {
        Some((
            "disk.critical",
            DISK_CRITICAL_POINTS,
            "Disk space is critically low",
        ))
    } else if below(DISK_LOW_FREE_PERCENT) {
        Some(("disk.low", DISK_LOW_POINTS, "Disk space is low"))
    } else if below(DISK_WATCH_FREE_PERCENT) {
        Some(("disk.watch", DISK_WATCH_POINTS, "Disk space is getting low"))
    } else {
        None
    }
}

fn disk_deduction(disk: &DiskSpace) -> Option<Deduction> {
    if disk.total_bytes == 0 {
        // A volume with no capacity has no free space to judge.
        return None;
    }
    let (id, points, title) = disk_tier(disk)?;
    let free = disk.free_bytes.min(disk.total_bytes);
    let reason_phrase = with_share(
        Phrase::new("health.disk")
            .bytes("free", free)
            .bytes("total", disk.total_bytes)
            .text("mount", disk.mount.trim_end_matches('\\')),
        free,
        disk.total_bytes,
    );
    Some(Deduction {
        id,
        check: HealthCheck::SystemDisk,
        points,
        title,
        reason: reason_phrase.english(),
        reason_phrase,
        fix: FixAction::OpenCleaner,
    })
}

fn memory_deduction(used_percent: f32) -> Option<Deduction> {
    let (id, points, title) = if used_percent >= MEMORY_CRITICAL_USED_PERCENT {
        (
            "memory.critical",
            MEMORY_CRITICAL_POINTS,
            "Memory is almost full",
        )
    } else if used_percent >= MEMORY_HIGH_USED_PERCENT {
        ("memory.high", MEMORY_HIGH_POINTS, "Memory use is high")
    } else {
        return None;
    };
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a percentage from 80 to 100, rounded for display"
    )]
    let percent = used_percent.round().max(0.0) as u64;
    let reason_phrase = Phrase::new("health.memory").count("percent", percent);
    Some(Deduction {
        id,
        check: HealthCheck::Memory,
        points,
        title,
        reason: reason_phrase.english(),
        reason_phrase,
        fix: FixAction::OpenMemory,
    })
}

fn restart_deduction() -> Deduction {
    let reason_phrase = Phrase::new("health.restart");
    Deduction {
        id: "restart.pending",
        check: HealthCheck::PendingRestart,
        points: RESTART_PENDING_POINTS,
        title: "A restart is waiting",
        reason: reason_phrase.english(),
        reason_phrase,
        fix: FixAction::RestartWindows,
    }
}

fn startup_deduction(load: StartupLoad) -> Option<Deduction> {
    let (id, points, title) = if load.enabled_apps >= STARTUP_MANY_APPS {
        (
            "startup.many",
            STARTUP_MANY_POINTS,
            "Many apps start with Windows",
        )
    } else if load.enabled_apps >= STARTUP_SOME_APPS {
        (
            "startup.some",
            STARTUP_SOME_POINTS,
            "Some apps slow down sign-in",
        )
    } else {
        return None;
    };
    let reason_phrase = Phrase::new("health.startup").count("count", load.enabled_apps);
    Some(Deduction {
        id,
        check: HealthCheck::Startup,
        points,
        title,
        reason: reason_phrase.english(),
        reason_phrase,
        fix: FixAction::OpenStartup,
    })
}

/// Adds `part` as a share of `whole` for display: one decimal below 10 percent (`5.0`), a whole
/// percentage from there (`24`).
fn with_share(phrase: Phrase, part: u64, whole: u64) -> Phrase {
    #[allow(
        clippy::cast_precision_loss,
        reason = "display only: one decimal of a share between 0 and 100"
    )]
    let share = part as f64 / whole as f64 * 100.0;
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a share between 0 and 100, rounded"
    )]
    if share < 10.0 {
        phrase.tenths("share", (share * 10.0).round() as u64)
    } else {
        phrase.count("share", share.round() as u64)
    }
}

/// Whether the mount `candidate` is the mount `system` (`C:` and `C:\` are the same drive;
/// case does not matter).
fn same_mount(candidate: &str, system: &str) -> bool {
    candidate
        .trim_end_matches('\\')
        .eq_ignore_ascii_case(system.trim_end_matches('\\'))
}

/// Reads the system once, read-only, and returns the inputs of the rules.
///
/// - Memory and the system disk come from one sample of `metrics`. `system_mount` names the disk
///   Windows is installed on (`C:` or `C:\`); when the sample fails or lacks that disk the
///   input is `None`.
/// - A pending restart is read through `registry` ([`pending_restart_markers`]).
///
/// A failed read is logged by class only (never its message) and the input stays `None`: it
/// shows up as unchecked, never as healthy.
pub fn read_inputs(
    registry: &dyn Registry,
    metrics: &dyn MetricsSource,
    system_mount: &str,
    startup_load: Option<StartupLoad>,
) -> HealthInputs {
    let (system_disk, memory_used_percent) = match metrics.read() {
        Ok(sample) => {
            let disk = sample
                .disks
                .iter()
                .filter_map(disk_snapshot)
                .find(|disk| same_mount(&disk.mount, system_mount))
                .map(|disk| DiskSpace::from(&disk));
            (disk, Some(memory_snapshot(sample.memory).used_percent))
        }
        Err(error) => {
            log::warn!(
                "health: the machine could not be sampled ({})",
                error_class(&error)
            );
            (None, None)
        }
    };
    let restart_pending = match pending_restart_markers(registry) {
        Ok(markers) => Some(!markers.is_empty()),
        Err(error) => {
            log::warn!(
                "health: the pending-restart markers could not be read ({})",
                error_class(&error)
            );
            None
        }
    };
    HealthInputs {
        system_disk,
        memory_used_percent,
        restart_pending,
        startup_load,
    }
}

/// Reads the system and applies the rules: [`read_inputs`] then [`assess`].
#[must_use]
pub fn inspect(
    registry: &dyn Registry,
    metrics: &dyn MetricsSource,
    system_mount: &str,
    startup_load: Option<StartupLoad>,
) -> HealthReport {
    assess(&read_inputs(registry, metrics, system_mount, startup_load))
}
