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

#![forbid(unsafe_code)]
