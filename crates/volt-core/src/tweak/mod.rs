//! The declarative tweak engine.
//!
//! A tweak is data: an id, labels (category, risk, evidence, restart) and an
//! ordered list of steps that map onto [`Operation`](crate::Operation)s
//! (registry values, services, commands). A [`Catalogue`] loads and validates
//! tweaks from TOML text; a [`TweakEngine`] reads their state, previews, applies
//! and reverts them through the [`Executor`](crate::Executor), so every change
//! is journaled before it is made. Adding a tweak to the catalogue needs no new
//! Rust code.
//!
//! Definitions also accept lower-case names where the core's enums serialize
//! with Rust variant names (`risk = "safe"`, `hive = "HKCU"`,
//! `value = { dword = 1 }`): the serde aliases on those enums exist for that.

mod admin_lint;
mod catalogue;
mod definition;
mod engine;
mod portable;

pub use catalogue::{Catalogue, CatalogueError, CatalogueIssue, IssueKind};
pub use definition::{Category, RestartRequirement, Tweak, TweakStep};
pub use engine::{
    ApplyOutcome, BatchRevert, Drift, RESET_LABEL_SUFFIX, StepPreview, TweakApplyReport,
    TweakEngine, TweakError, TweakRevertReport, TweakState,
};
pub use portable::{
    CONFIG_FORMAT, CONFIG_VERSION, ConfigError, ConfigFile, ConfigTweak, ImportAction, ImportSkip,
    MAX_CONFIG_BYTES, MAX_CONFIG_TWEAKS, TweakImport,
};

#[cfg(test)]
mod tests;
