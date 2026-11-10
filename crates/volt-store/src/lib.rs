//! Volt store: persistence adapters for the `volt-core` ports (undo journal,
//! settings, activity log).
//!
//! Dependency rule (enforced by `scripts/check-arch.mjs`): depends on
//! `volt-core` only; must not pull in `tauri`, `windows` or `windows-sys`.
//!
//! [`Store`] is one `SQLite` database that implements the core's
//! [`Journal`](volt_core::Journal), [`Settings`](volt_core::Settings) and
//! [`ActivityLog`](volt_core::ActivityLog) ports. The caller decides where the file lives; this crate never picks a
//! location.
//!
//! # Durability
//!
//! The undo guarantee rests on `record_pending` being durable before it
//! returns. The database runs in WAL mode with `synchronous = FULL`, and every
//! journal write is a single auto-committed statement, so `SQLite` has fsynced
//! the write-ahead log before the call returns. Even if the filesystem cannot
//! do WAL, `synchronous = FULL` keeps the rollback journal fsynced too.
//!
//! # Corruption
//!
//! A database that is not a valid `SQLite` file, or that fails an integrity
//! check, is never silently replaced and never deleted: [`Store::open`] moves
//! it (and its `-wal` / `-shm` files) aside to a timestamped backup, starts a
//! fresh database and reports the backup path in [`OpenOutcome::recovery`].

#![forbid(unsafe_code)]

mod activity;
mod error;
mod journal;
mod migrations;
mod settings;
mod store;

pub use error::StoreError;
pub use store::{CorruptionRecovery, OpenOutcome, Store};

#[cfg(test)]
mod tests;
