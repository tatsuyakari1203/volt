//! One-click Boost: the default memory operations plus a read-only junk scan, in one use case.
//!
//! Boost is a composition of two engines that already exist, with a small configuration:
//!
//! - the **memory operations** the user chose (working-set trim by default) run through the
//!   [`MemoryCleaner`]. An operation that cannot run in this session (administrator rights, an
//!   unsupported build) is not attempted and is listed in [`BoostReport::skipped`] with the
//!   reason;
//! - a **read-only scan** of the Safe cleaning rules that need no administrator rights says how
//!   much could be cleaned. It changes nothing: cleaning shows a preview first, and Boost has no
//!   preview;
//! - only when the user opted in ([`BoostConfig::move_temp_to_recycle_bin`]) Boost also moves the
//!   result of the user temp rule to the Recycle Bin, and no other rule. The report then says how
//!   many bytes were *moved to the Recycle Bin*; it never claims disk space was freed.
//!
//! Everything it changes is recorded by the engines it uses (the memory run and each cleaned rule
//! write their own activity record), so there is nothing extra to keep in step.

use serde::{Deserialize, Serialize};

use crate::clean::{CleanEngine, CleanReport};
use crate::error::PortError;
use crate::journal::Source;
use crate::label::Risk;
use crate::memory::{MemoryCleanReport, MemoryCleaner};
use crate::ports::{Availability, MemoryArea, MemoryLists};
use crate::settings::{Settings, SettingsExt};

/// The settings key that holds the [`BoostConfig`].
pub const BOOST_SETTINGS_KEY: &str = "boost.config";

/// The one cleaning rule Boost may clean, when the user opted in.
pub const BOOST_TEMP_RULE_ID: &str = "user-temp";

/// What Boost includes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BoostConfig {
    /// The memory operations that run. Each runs at most once, in the order Windows needs.
    pub areas: Vec<MemoryArea>,
    /// Also move the user temp rule's files to the Recycle Bin, without a preview. Off by default.
    pub move_temp_to_recycle_bin: bool,
}

impl Default for BoostConfig {
    /// The working-set trim only (it needs no administrator rights); nothing is cleaned.
    fn default() -> Self {
        Self {
            areas: vec![MemoryArea::WorkingSets],
            move_temp_to_recycle_bin: false,
        }
    }
}

impl BoostConfig {
    /// The stored configuration, or the default when none is stored. A stored value that cannot
    /// be read or has the wrong shape also gives the default (and a warning): never a setting
    /// that turns cleaning on.
    #[must_use]
    pub fn load(settings: &dyn Settings) -> Self {
        match settings.get_typed::<Self>(BOOST_SETTINGS_KEY) {
            Ok(Some(config)) => config,
            Ok(None) => Self::default(),
            Err(error) => {
                log::warn!(
                    "the Boost settings could not be read ({}); using the defaults",
                    crate::activity::error_class(&error)
                );
                Self::default()
            }
        }
    }

    /// Stores the configuration.
    pub fn save(&self, settings: &dyn Settings) -> Result<(), PortError> {
        settings.set_typed(BOOST_SETTINGS_KEY, self)
    }
}

/// A selected memory operation that was not run, and why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SkippedOperation {
    /// The operation.
    pub area: MemoryArea,
    /// Why it could not run.
    pub reason: Availability,
}

/// What the read-only scan found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct JunkFound {
    /// Files that could be moved to the Recycle Bin.
    pub files: usize,
    /// Their size in bytes.
    pub bytes: u64,
    /// Whether a rule stopped at its item cap, so the figures are a lower bound.
    pub truncated: bool,
}

/// What one Boost did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoostReport {
    /// The memory run (no areas, and no readings, when nothing ran).
    pub memory: MemoryCleanReport,
    /// Selected operations that were not run, with the reason.
    pub skipped: Vec<SkippedOperation>,
    /// What the Safe rules could clean, or `None` when there is no clean engine.
    pub junk: Option<JunkFound>,
    /// What the opt-in temp cleaning moved, or `None` when it did not run.
    pub cleaned: Option<CleanReport>,
}

impl BoostReport {
    /// Memory available before and after the run, when both readings exist. Available memory is
    /// what a program can get at once: free pages plus the standby cache.
    #[must_use]
    pub fn memory_available(&self) -> Option<(u64, u64)> {
        Some((
            available(self.memory.before?),
            available(self.memory.after?),
        ))
    }

    /// Bytes moved to the Recycle Bin, when cleaning ran. Not freed space.
    #[must_use]
    pub fn moved_to_bin_bytes(&self) -> Option<u64> {
        self.cleaned.as_ref().map(CleanReport::moved_bytes)
    }
}

/// Free pages plus the standby cache.
#[must_use]
pub fn available(lists: MemoryLists) -> u64 {
    lists.free_bytes.saturating_add(lists.standby_bytes)
}

/// Runs Boost as `config` says. `clean` is `None` when the build has no clean engine; the junk
/// scan and the cleaning are then left out. Never fails: each part reports its own outcome.
#[must_use]
pub fn run_boost(
    memory: &MemoryCleaner,
    clean: Option<&CleanEngine>,
    config: &BoostConfig,
    source: Source,
) -> BoostReport {
    let mut runnable = Vec::new();
    let mut skipped = Vec::new();
    for (area, availability) in memory.availability() {
        if !config.areas.contains(&area) {
            continue;
        }
        if availability == Availability::Available {
            runnable.push(area);
        } else {
            skipped.push(SkippedOperation {
                area,
                reason: availability,
            });
        }
    }
    let memory_report = memory.run(&runnable, source);
    let (junk, cleaned) = clean.map_or((None, None), |engine| {
        scan_and_clean(engine, config, source)
    });
    BoostReport {
        memory: memory_report,
        skipped,
        junk,
        cleaned,
    }
}

fn scan_and_clean(
    engine: &CleanEngine,
    config: &BoostConfig,
    source: Source,
) -> (Option<JunkFound>, Option<CleanReport>) {
    let ids: Vec<String> = engine
        .rules()
        .rules()
        .iter()
        .filter(|rule| rule.risk == Risk::Safe && !rule.requires_admin)
        .map(|rule| rule.id.clone())
        .collect();
    let plan = match engine.scan(&ids) {
        Ok(plan) => plan,
        Err(error) => {
            log::warn!("the Boost junk scan could not run: {error}");
            return (None, None);
        }
    };
    let junk = JunkFound {
        files: plan.total_files(),
        bytes: plan.total_bytes(),
        truncated: plan.rules.iter().any(|rule| rule.truncated),
    };
    let temp_has_items = plan
        .rules
        .iter()
        .any(|rule| rule.rule_id == BOOST_TEMP_RULE_ID && !rule.items.is_empty());
    let cleaned = (config.move_temp_to_recycle_bin && temp_has_items)
        .then(|| engine.clean(&plan.confirm_rules(&[BOOST_TEMP_RULE_ID]), source));
    (Some(junk), cleaned)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use super::*;
    use crate::clean::CleaningRules;
    use crate::clean::Location;
    use crate::clean::fakes::FakeKnownFolders;
    use crate::fakes::{FakeActivityLog, FakeFileSystem, FakeMemory, FakeSettings, ManualClock};
    use crate::time::Timestamp;

    const NOW: i64 = 1_800_000_000_000;
    const DAY: i64 = 86_400_000;

    const RULES: &str = r#"
[[rule]]
id = "user-temp"
title = "Temporary files"
description = "Files in the temp folder."
category = "temporary_files"
risk = "safe"
include = ["**"]
[rule.root]
location = "user_temp"
[rule.constraints]
min_age_days = 1
max_depth = 2

[[rule]]
id = "thumbnails"
title = "Thumbnails"
description = "Thumbnail cache."
category = "application_cache"
risk = "safe"
include = ["thumbcache_*.db"]
[rule.root]
location = "local_app_data"
path = "Cache"
[rule.constraints]
min_age_days = 0
max_depth = 0

[[rule]]
id = "caution-rule"
title = "Caution"
description = "A caution rule."
category = "logs"
risk = "caution"
include = ["**"]
[rule.root]
location = "local_app_data"
path = "Logs"
[rule.constraints]
min_age_days = 0
max_depth = 1

[[rule]]
id = "machine-rule"
title = "Machine"
description = "Needs administrator rights."
category = "logs"
risk = "safe"
include = ["**"]
requires_admin = true
[rule.root]
location = "windows"
path = "Temp"
[rule.constraints]
min_age_days = 0
max_depth = 1
"#;

    fn abs(rest: &str) -> PathBuf {
        let base = if cfg!(windows) { "C:\\" } else { "/" };
        Path::new(base).join(rest)
    }

    struct Fixture {
        memory: Arc<FakeMemory>,
        fs: Arc<FakeFileSystem>,
        activity: Arc<FakeActivityLog>,
        cleaner: MemoryCleaner,
        engine: CleanEngine,
    }

    fn fixture() -> Fixture {
        let folders = Arc::new(FakeKnownFolders::new());
        folders.set(Location::UserTemp, abs("Users/t/Temp"));
        folders.set(Location::LocalAppData, abs("Users/t/Local"));
        folders.set(Location::Windows, abs("Windows"));
        folders.protect(abs("Users/t"));
        folders.protect(abs("Windows"));
        let fs = Arc::new(FakeFileSystem::new());
        let old = Timestamp::from_unix_millis(NOW - 10 * DAY);
        fs.add_file_at(abs("Users/t/Temp/a.tmp"), 1000, old);
        fs.add_file_at(abs("Users/t/Temp/b.tmp"), 2000, old);
        fs.add_file_at(abs("Users/t/Local/Cache/thumbcache_32.db"), 500, old);
        fs.add_file_at(abs("Users/t/Local/Logs/x.log"), 700, old);
        fs.add_file_at(abs("Windows/Temp/m.tmp"), 900, old);
        let clock = Arc::new(ManualClock::new(NOW));
        let activity = Arc::new(FakeActivityLog::new());
        let memory = Arc::new(FakeMemory::new());
        let engine = CleanEngine::new(
            CleaningRules::from_toml(RULES).expect("valid rules"),
            folders,
            fs.clone(),
            clock.clone(),
        )
        .with_activity_log(activity.clone());
        let cleaner = MemoryCleaner::new(memory.clone(), clock).with_activity_log(activity.clone());
        Fixture {
            memory,
            fs,
            activity,
            cleaner,
            engine,
        }
    }

    #[test]
    fn the_default_boost_trims_working_sets_and_only_scans_for_junk() {
        let f = fixture();
        let report = run_boost(
            &f.cleaner,
            Some(&f.engine),
            &BoostConfig::default(),
            Source::Manual,
        );
        assert_eq!(f.memory.purged(), [MemoryArea::WorkingSets]);
        assert!(report.skipped.is_empty());
        let (before, after) = report.memory_available().expect("both readings");
        assert!(after >= before);
        // Safe, non-administrator rules only: temp (3000) and thumbnails (500).
        let junk = report.junk.expect("scanned");
        assert_eq!((junk.files, junk.bytes), (3, 3500));
        assert!(report.cleaned.is_none());
        assert!(f.fs.trashed().is_empty(), "the default boost never cleans");
        // Only the memory run is recorded: a scan changes nothing.
        assert_eq!(f.activity.records().len(), 1);
    }

    #[test]
    fn an_operation_that_cannot_run_is_skipped_with_its_reason_and_never_attempted() {
        let f = fixture();
        f.memory
            .make_unavailable(MemoryArea::StandbyList, Availability::NeedsAdministrator);
        let config = BoostConfig {
            areas: vec![MemoryArea::StandbyList, MemoryArea::WorkingSets],
            move_temp_to_recycle_bin: false,
        };
        let report = run_boost(&f.cleaner, None, &config, Source::Manual);
        assert_eq!(f.memory.purged(), [MemoryArea::WorkingSets]);
        assert_eq!(
            report.skipped,
            [SkippedOperation {
                area: MemoryArea::StandbyList,
                reason: Availability::NeedsAdministrator
            }]
        );
        assert!(report.junk.is_none() && report.cleaned.is_none());
    }

    #[test]
    fn the_opt_in_moves_the_temp_rule_only_and_reports_bytes_moved() {
        let f = fixture();
        let config = BoostConfig {
            areas: Vec::new(),
            move_temp_to_recycle_bin: true,
        };
        let report = run_boost(&f.cleaner, Some(&f.engine), &config, Source::Manual);
        let mut trashed = f.fs.trashed();
        trashed.sort();
        assert_eq!(
            trashed,
            [abs("Users/t/Temp/a.tmp"), abs("Users/t/Temp/b.tmp")]
        );
        assert_eq!(report.moved_to_bin_bytes(), Some(3000));
        assert!(f.memory.purged().is_empty());
        assert!(report.memory.areas.is_empty() && report.memory_available().is_none());
        // The scan still reports everything that could be cleaned.
        assert_eq!(report.junk.map(|junk| junk.files), Some(3));
    }

    #[test]
    fn the_config_round_trips_and_a_bad_stored_value_falls_back_to_the_safe_default() {
        let settings = FakeSettings::new();
        assert_eq!(BoostConfig::load(&settings), BoostConfig::default());
        let config = BoostConfig {
            areas: vec![MemoryArea::FileCache],
            move_temp_to_recycle_bin: true,
        };
        config.save(&settings).expect("saved");
        assert_eq!(BoostConfig::load(&settings), config);
        settings
            .set(BOOST_SETTINGS_KEY, &serde_json::json!("garbage"))
            .expect("set");
        let fallback = BoostConfig::load(&settings);
        assert_eq!(fallback, BoostConfig::default());
        assert!(!fallback.move_temp_to_recycle_bin);
    }
}
