//! Volt core: the domain model, use cases and port traits.
//!
//! This crate is the centre of the dependency graph. It describes *what* Volt
//! does (plan, preview, apply and revert system changes) and declares the
//! traits ("ports") it needs from the outside world. It never talks to
//! Windows, the UI or a database directly, so everything here can be tested
//! with in-memory fakes on any platform.
//!
//! Dependency rule (enforced by `scripts/check-arch.mjs`): this crate depends
//! on no other workspace crate and must not pull in `tauri`, `windows`,
//! `windows-sys`, `winreg` or `rusqlite`.
//!
//! # The model in one paragraph
//!
//! An [`Operation`] is declarative data describing one undoable change. A
//! [`Plan`] is a list of operations, each with a [`Preview`], a [`Risk`] and an
//! [`Evidence`] label; building a plan only reads the system, so it is the dry
//! run. The [`Executor`] applies a plan and enforces the safety property that
//! callers cannot bypass: *write-ahead journaling*. For every operation it
//! detects the prior state, has the [`Journal`] record a pending entry that
//! contains that prior state, only then mutates the system, and finally marks
//! the entry applied. Reverting restores the recorded prior state exactly,
//! including "the value did not exist".
//!
//! On top of that, a [`Tweak`] is data (loaded from a TOML [`Catalogue`]) and
//! the [`TweakEngine`] reads its state, previews, applies and reverts it
//! through the executor.
//!
//! The health score (the [`health`] module) is a pure function of a few read-only inputs and
//! lists every deduction with its reason, so the number never changes without a visible cause.
//! Startup entries are managed by the [`startup`] module: it lists what starts
//! with Windows and switches entries on and off through the same executor, with
//! the `StartupApproved` records Task Manager uses.
//!
//! Cleaning is its own engine (the [`clean`] module): rules are data, a scan
//! is read-only and returns a plan, and only a confirmed plan can be cleaned,
//! always into the Recycle Bin.
//!
//! Memory cleaning is the [`memory`] module: each purge (working sets, the standby, low-priority
//! standby and modified lists, the file cache) is a separate operation over the `Memory` port, a run
//! reports memory by list before and after, and it is recorded as an activity that cannot be
//! undone (it releases reclaimable memory; there is no earlier value to restore).
//!
//! The process list and ending an app are the [`process`] module: processes are grouped by
//! executable name, CPU load comes from two samples, and ending a group refuses what Windows
//! needs and Volt itself (in the core, not only in the UI) and is recorded as an activity that
//! cannot be undone.
//!
//! Priority, CPU affinity and Efficiency mode of running apps are the [`tuning`] module: a change
//! is one undoable operation over the processes of an app, and saved rules reapply it to the
//! processes that start later, on a tick the shell already has.
//!
//! Installed apps are the [`apps`] module: desktop apps from the Uninstall registry keys and the
//! packaged apps of the current user are listed read-only, and removing one starts the vendor's
//! own registered uninstaller (or asks the package manager) and is recorded as an activity that
//! cannot be undone.
//!
//! The elevated helper's wire protocol and the [`Elevator`] port live in the [`elevation`] module:
//! a closed set of requests, bounded messages and strictly ordered ids, so the UI side can be built
//! and tested without a helper.
//!
//! # Fakes
//!
//! In-memory fakes of every port live in the `fakes` module, compiled for this
//! crate's own tests and for other crates that enable the `fakes` feature.

#![forbid(unsafe_code)]

pub mod activity;
pub mod apps;
pub mod boost;
pub mod clean;
pub mod command;
pub mod elevation;
pub mod error;
pub mod exclusions;
pub mod executor;
pub mod health;
pub mod journal;
pub mod label;
pub mod large_files;
pub mod memory;
pub mod metrics;
pub mod operation;
pub mod phrase;
pub mod plan;
pub mod ports;
pub mod process;
pub mod registry;
pub mod service;
pub mod settings;
pub mod startup;
pub mod tidy;
pub mod time;
pub mod trend;
pub mod tuning;
pub mod tweak;

#[cfg(any(test, feature = "fakes"))]
pub mod fakes;

pub use activity::{
    ActivityId, ActivityLog, ActivityOutcome, ActivityRecord, NewActivity, Retention,
    redacted_entry_error, render_text,
};
pub use apps::{
    AppDate, AppListing, AppLocation, AppManager, AppProblem, AppSource, CommandRefusal,
    HiddenReason, InstalledApp, OwnInstall, PackagedApp, PackagedApps, StopWaiting,
    UNINSTALL_LABEL, UninstallCommand, UninstallError, UninstallOutcome, UninstallReport,
    Uninstaller, UninstallerExit, Waited,
};
pub use boost::{
    BOOST_SETTINGS_KEY, BOOST_TEMP_RULE_ID, BoostConfig, BoostReport, JunkFound, SkippedOperation,
    run_boost,
};
pub use clean::{
    CleanCategory, CleanEngine, CleanItem, CleanPlan, CleanReport, CleanSkip, CleanSkipReason,
    CleaningRule, CleaningRules, ConfirmedPlan, EmptyReport, FolderError, ItemKind, ItemUnit,
    KnownFolders, Location, RecycleBin, RecycleBinManager, RecycleBinStatus, RuleClean, RuleScan,
    RuleSelection, RulesError, ScanError, ScanFailure, ScanObserver, SelectionError, SkipReason,
};
pub use command::{Command, CommandOutput, Commands, MAX_TIMEOUT_SECS};
pub use elevation::confirm::{
    ConfirmAction, ConfirmGate, ConfirmRequest, Confirmer, DENIAL_BLOCK, DENIALS_BEFORE_LOCKOUT,
    Decision, Verdict,
};
pub use elevation::mirror::{HelperMirror, MirrorRow, RowState, pull as pull_helper_history};
pub use elevation::restore::{
    RESTORE_POINT_MIN_GAP, RestoreGate, RestorePoints, needs_restore_point,
};
pub use elevation::{
    AreaResult, AreaStatus, CatalogueHashes, ElevationError, ElevationStatus, Elevator, ErrorCode,
    HISTORY_MAX_RECORDS, HelloReply, HistoryReply, MAX_REPLY_BYTES, MAX_REQUEST_BYTES,
    MemoryPurgeReply, PROTOCOL_VERSION, Reply, ReplyBody, Request, RequestEnvelope,
    RequestSequencer, TweakApplyReply, TweakRevertReply, UnavailableReason, catalogue_hash,
    decode_reply, decode_request, encode_reply, encode_request,
};
pub use error::{ExecError, PlanError, PortError, RevertError};
pub use exclusions::{EXCLUSIONS_SETTINGS_KEY, ExclusionList, MAX_EXCLUSIONS};
pub use executor::{
    ApplyReport, Executor, ItemOutcome, ItemStatus, ReconcileOutcome, ReconcileReport, Resolution,
    RevertOutcome, RevertReport,
};
pub use health::{
    Deduction, DiskSpace, FixAction, HealthCheck, HealthInputs, HealthLevel, HealthReport,
    RestartMarker, StartupLoad,
};
pub use journal::{
    BatchId, EntryId, EntryState, Journal, JournalEntry, NewEntry, Overwrite, Source, overwrites_of,
};
pub use label::{Evidence, Risk};
pub use large_files::{
    DEFAULT_MAX_FILES, FileGroup, LargeFile, LargeFilesError, LargeFilesFinder, LargeFilesObserver,
    LargeFilesScan, LargeFilesTrash, ScanFilter,
};
pub use memory::{AreaReport, MEMORY_LABEL, MemoryCleanReport, MemoryCleaner};
pub use metrics::{
    CpuTimes, DiskReading, DiskSnapshot, GpuReading, GpuSnapshot, MemorySnapshot, MetricsSnapshot,
    MetricsSource, MetricsTracker, RawSample,
};
pub use operation::{Operation, PriorState};
pub use phrase::{Arg, Phrase, class_phrase};
pub use plan::{OperationRequest, Plan, PlanItem, Preview};
pub use ports::{
    Availability, DirEntry, EntryInfo, EntryKind, FileIdentity, FileStat, FileSystem, Memory,
    MemoryArea, MemoryLists, MemoryStatus, Ports, ProcessInfo, Processes, PurgeOutcome, Services,
};
pub use process::{
    EndError, EndReport, EndStatus, Endability, PROCESS_END_LABEL, ProcessEnd, ProcessGroup,
    ProcessMonitor, ProcessSample, Protection, is_windows_critical,
};
pub use registry::{
    NamedValue, Registry, RegistryHive, RegistryKeyContents, RegistryKeyRef, RegistryValue,
    RegistryValueRef, RegistryView,
};
pub use service::ServiceStartType;
pub use settings::{Settings, SettingsExt};
pub use startup::{
    ChangeOutcome, FilePublisher, LogonTask, LogonTasks, NoPublisher, ScheduledTaskEntry,
    ShortcutResolver, ShortcutTarget, SourceProblem, StartupChange, StartupEntry, StartupError,
    StartupListing, StartupManager, StartupOrigin, StartupScope, StartupSources, StartupState,
};
pub use tidy::{
    BrokenShortcut, ContentHash, DEFAULT_MIN_DUPLICATE_BYTES, DuplicateFile, DuplicateGroup,
    DuplicatePick, EmptyFolder, TidyFinder, TidyMoved, TidyObserver, TidyOptions, TidyProgress,
    TidyScan, TidyStage, TidyTrash,
};
pub use time::{Clock, SystemClock, Timestamp};
pub use trend::{MemoryHistory, MemoryPoint};
pub use tuning::{
    Inspection, MAX_RULES, PowerThrottling, PriorityClass, ProcessChange, ProcessRule,
    ProcessSettings, ProcessTarget, ProcessTuner, RULES_SETTINGS_KEY, RuleStatus, RulesRun,
    TUNE_LABEL, TuneError, TuneReport, TuneRequest,
};
pub use tweak::{
    ApplyOutcome, BatchRevert, CONFIG_FORMAT, CONFIG_VERSION, Catalogue, CatalogueError,
    CatalogueIssue, Category, ConfigError, ConfigFile, ConfigTweak, Drift, ImportAction,
    ImportSkip, IssueKind, MAX_CONFIG_BYTES, MAX_CONFIG_TWEAKS, RESET_LABEL_SUFFIX,
    RestartRequirement, StepPreview, Tweak, TweakApplyReport, TweakEngine, TweakError, TweakImport,
    TweakRevertReport, TweakState, TweakStep,
};
