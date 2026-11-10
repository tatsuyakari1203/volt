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
//! Cleaning is its own engine (the [`clean`] module): rules are data, a scan
//! is read-only and returns a plan, and only a confirmed plan can be cleaned,
//! always into the Recycle Bin.
//!
//! # Fakes
//!
//! In-memory fakes of every port live in the `fakes` module, compiled for this
//! crate's own tests and for other crates that enable the `fakes` feature.

#![forbid(unsafe_code)]

pub mod activity;
pub mod clean;
pub mod command;
pub mod error;
pub mod executor;
pub mod journal;
pub mod label;
pub mod metrics;
pub mod operation;
pub mod plan;
pub mod ports;
pub mod registry;
pub mod service;
pub mod settings;
pub mod time;
pub mod tweak;

#[cfg(any(test, feature = "fakes"))]
pub mod fakes;

pub use activity::{
    ActivityId, ActivityLog, ActivityOutcome, ActivityRecord, NewActivity, render_text,
};
pub use clean::{
    CleanCategory, CleanEngine, CleanItem, CleanPlan, CleanReport, CleanSkip, CleanSkipReason,
    CleaningRule, CleaningRules, ConfirmedPlan, ItemKind, KnownFolders, Location, RuleClean,
    RuleScan, RulesError, ScanFailure, SkipReason,
};
pub use command::{Command, CommandOutput, Commands};
pub use error::{ExecError, PlanError, PortError, RevertError};
pub use executor::{
    ApplyReport, Executor, ItemOutcome, ItemStatus, ReconcileOutcome, ReconcileReport, Resolution,
    RevertOutcome, RevertReport,
};
pub use journal::{BatchId, EntryId, EntryState, Journal, JournalEntry, NewEntry, Source};
pub use label::{Evidence, Risk};
pub use metrics::{
    CpuTimes, DiskReading, DiskSnapshot, GpuReading, GpuSnapshot, MemorySnapshot, MetricsSnapshot,
    MetricsSource, MetricsTracker, RawSample,
};
pub use operation::{Operation, PriorState};
pub use plan::{OperationRequest, Plan, PlanItem, Preview};
pub use ports::{
    DirEntry, EntryInfo, EntryKind, FileStat, FileSystem, Memory, MemoryArea, MemoryStatus, Ports,
    ProcessInfo, Processes, Services,
};
pub use registry::{Registry, RegistryHive, RegistryValue, RegistryValueRef, RegistryView};
pub use service::ServiceStartType;
pub use settings::{Settings, SettingsExt};
pub use time::{Clock, SystemClock, Timestamp};
pub use tweak::{
    ApplyOutcome, BatchRevert, Catalogue, CatalogueError, CatalogueIssue, Category, IssueKind,
    RestartRequirement, Tweak, TweakApplyReport, TweakEngine, TweakError, TweakRevertReport,
    TweakState, TweakStep,
};
