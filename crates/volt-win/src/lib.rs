//! Volt Windows adapters: implementations of the `volt-core` ports on top of
//! the Windows API (registry, services, processes, memory lists, file system).
//!
//! This is the only crate in the workspace that may contain `unsafe` code.
//! Every `unsafe` block needs a `// SAFETY:` comment explaining why the call
//! is sound (clippy's `undocumented_unsafe_blocks` enforces this).
//!
//! Dependency rule (enforced by `scripts/check-arch.mjs`): depends on
//! `volt-core` only; must not pull in `tauri` or `rusqlite`.
//!
//! Adapters in this crate: [`WinRegistry`], [`WinServices`], [`WinFileSystem`],
//! [`WinKnownFolders`], [`WinRecycleBin`], [`WinMetrics`], [`WinMemory`], [`WinProcesses`],
//! [`WinCommands`], [`WinPackagedApps`], [`WinUninstaller`], [`WinFilePublisher`], [`WinShortcuts`], [`WinLogonTasks`], [`WinAutoStartServices`],
//! [`WinElevator`], plus [`display_languages`] (the user's display language, for the UI) and
//! [`machine_ui_language_id`] (the machine's, for the helper).
//! The [`helper`] module holds the Windows side of the elevated helper: its named pipe, the checks on
//! who may use it, the install-folder check and the helper's self-hardening.
//! The crate is empty on non-Windows targets.

// The workspace denies unsafe code; FFI lives here by design.
#![allow(unsafe_code)]
#![cfg(windows)]

mod apps;
mod autostart_services;
mod command;
mod environment;
mod error;
mod files;
mod folders;
mod gpu;
pub mod helper;
mod language;
mod memory;
mod metrics;
mod processes;
mod recycle_bin;
mod registry;
mod services;
mod startup_sources;
mod version;
mod wide;

pub use apps::{WinPackagedApps, WinUninstaller};
pub use autostart_services::WinAutoStartServices;
pub use command::{DEFAULT_MAX_OUTPUT, DEFAULT_TIMEOUT, WinCommands};
pub use environment::WinEnvironment;
pub use files::WinFileSystem;
pub use folders::WinKnownFolders;
pub use helper::elevator::WinElevator;
pub use language::{LANG_VIETNAMESE, display_languages, machine_ui_language_id, primary_language};
pub use memory::WinMemory;
pub use metrics::WinMetrics;
pub use processes::WinProcesses;
pub use recycle_bin::WinRecycleBin;
pub use registry::WinRegistry;
pub use services::WinServices;
pub use startup_sources::{WinLogonTasks, WinShortcuts};
pub use version::WinFilePublisher;
