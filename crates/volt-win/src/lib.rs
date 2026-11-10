//! Volt Windows adapters: implementations of the `volt-core` ports on top of
//! the Windows API (registry, services, processes, memory lists, file system).
//!
//! This is the only crate in the workspace that may contain `unsafe` code.
//! Every `unsafe` block needs a `// SAFETY:` comment explaining why the call
//! is sound (clippy's `undocumented_unsafe_blocks` enforces this).
//!
//! Dependency rule (enforced by `scripts/check-arch.mjs`): depends on
//! `volt-core` only; must not pull in `tauri` or `rusqlite`.

// The workspace denies unsafe code; FFI lives here by design.
#![allow(unsafe_code)]
