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
pub mod boost;
pub mod clean;
pub mod command;
pub mod elevation;
pub mod error;
pub mod executor;
pub mod health;
pub mod journal;
pub mod label;
pub mod large_files;
pub mod memory;
pub mod metrics;
pub mod operation;
pub mod plan;
pub mod ports;
pub mod process;
pub mod registry;
pub mod service;
pub mod settings;
pub mod startup;
pub mod time;
pub mod tweak;

#[cfg(any(test, feature = "fakes"))]
pub mod fakes;

pub use activity::{
    ActivityId, ActivityLog, ActivityOutcome, ActivityRecord, NewActivity, Retention,
    redacted_entry_error, render_text,
};
pub use boost::{
    BOOST_SETTINGS_KEY, BOOST_TEMP_RULE_ID, BoostConfig, BoostReport, JunkFound, SkippedOperation,
    run_boost,
};
pub use clean::{
    CleanCategory, CleanEngine, CleanItem, CleanPlan, CleanReport, CleanSkip, CleanSkipReason,
    CleaningRule, CleaningRules, ConfirmedPlan, EmptyReport, ItemKind, KnownFolders, Location,
    RecycleBin, RecycleBinManager, RecycleBinStatus, RuleClean, RuleScan, RulesError, ScanError,
    ScanFailure, ScanObserver, SkipReason,
};
pub use command::{Command, CommandOutput, Commands, MAX_TIMEOUT_SECS};
pub use elevation::{
    AreaResult, AreaStatus, CatalogueHashes, ElevationError, ElevationStatus, Elevator, ErrorCode,
    HelloReply, MAX_REPLY_BYTES, MAX_REQUEST_BYTES, MemoryPurgeReply, PROTOCOL_VERSION, Reply,
    ReplyBody, Request, RequestEnvelope, RequestSequencer, UnavailableReason, catalogue_hash,
    decode_reply, decode_request, encode_reply, encode_request,
};
pub use error::{ExecError, PlanError, PortError, RevertError};
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
    LargeFilesScan, LargeFilesTrash,
};
pub use memory::{AreaReport, MEMORY_LABEL, MemoryCleanReport, MemoryCleaner};
pub use metrics::{
    CpuTimes, DiskReading, DiskSnapshot, GpuReading, GpuSnapshot, MemorySnapshot, MetricsSnapshot,
    MetricsSource, MetricsTracker, RawSample,
};
pub use operation::{Operation, PriorState};
pub use plan::{OperationRequest, Plan, PlanItem, Preview};
pub use ports::{
    Availability, DirEntry, EntryInfo, EntryKind, FileStat, FileSystem, Memory, MemoryArea,
    MemoryLists, MemoryStatus, Ports, ProcessInfo, Processes, PurgeOutcome, Services,
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
pub use time::{Clock, SystemClock, Timestamp};
pub use tweak::{
    ApplyOutcome, BatchRevert, Catalogue, CatalogueError, CatalogueIssue, Category, IssueKind,
    RESET_LABEL_SUFFIX, RestartRequirement, StepPreview, Tweak, TweakApplyReport, TweakEngine,
    TweakError, TweakRevertReport, TweakState, TweakStep,
};
