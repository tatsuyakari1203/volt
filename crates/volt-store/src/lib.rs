//! Volt store: persistence adapters for the `volt-core` ports (undo journal,
//! settings, activity log).
//!
//! Dependency rule (enforced by `scripts/check-arch.mjs`): depends on
//! `volt-core` only; must not pull in `tauri`, `windows` or `windows-sys`.

#![forbid(unsafe_code)]
