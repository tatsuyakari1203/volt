//! The composition root: where the adapters are wired into the core.
//!
//! [`build_core`] is the one place that knows every crate. It opens the [`Store`] once (the
//! undo journal, the settings and the activity log are the same database), builds the executor
//! and the tweak engine over the given [`Ports`], and resolves entries a crash left pending.
//! It takes the ports and the store path as parameters, so the app calls it with the real
//! Windows adapters ([`windows_ports`]) and a path in the app data directory, and tests call
//! the very same function with fakes or a throwaway registry key and a temp directory.
//!
//! The result, [`AppCore`], lives in Tauri managed state ([`AppState`]) and so outlives the
//! window: the window and its `WebView` come and go, the engine and the database connection stay.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tauri::{AppHandle, Manager as _};
use volt_core::{
    ActivityLog, Catalogue, CatalogueError, Clock, Executor, Journal, PortError, Ports,
    ReconcileReport, Resolution, Settings, SystemClock, TweakEngine,
};
use volt_store::{CorruptionRecovery, OpenOutcome, Store, StoreError};
use volt_win::{WinCommands, WinFileSystem, WinRegistry, WinServices};

use crate::IpcError;
use crate::catalogue::EMBEDDED_CATALOGUE;
use crate::unsupported::{UnsupportedMemory, UnsupportedProcesses};

/// File name of the database inside the app data directory.
pub const STORE_FILE_NAME: &str = "volt.db";

/// Why the core could not be built.
#[derive(Debug)]
#[non_exhaustive]
pub enum InitError {
    /// The database could not be opened.
    Store(StoreError),
    /// The catalogue text is invalid. The embedded catalogue is tested, so this is a build bug.
    Catalogue(CatalogueError),
    /// The app data directory could not be found or created.
    DataDir(String),
}

impl fmt::Display for InitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(error) => write!(f, "{error}"),
            Self::Catalogue(error) => write!(f, "{error}"),
            Self::DataDir(message) => write!(f, "cannot use the app data directory: {message}"),
        }
    }
}

impl std::error::Error for InitError {}

impl From<StoreError> for InitError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl From<CatalogueError> for InitError {
    fn from(error: CatalogueError) -> Self {
        Self::Catalogue(error)
    }
}

/// What happened while the core was being built, for logs and tests.
#[derive(Debug)]
pub struct StartupReport {
    /// The database file in use.
    pub store_path: PathBuf,
    /// `Some` when the database was corrupt: it was moved aside (never deleted) and a fresh one
    /// was started, so earlier changes cannot be undone from Volt any more.
    pub recovery: Option<CorruptionRecovery>,
    /// What resolving entries left pending by a crash found, or why that could not be done.
    pub reconcile: Result<ReconcileReport, PortError>,
}

/// The wired core: the tweak engine and the stores behind it.
pub struct AppCore {
    /// Reads, applies and reverts the catalogue tweaks, journaling and recording every change.
    pub engine: TweakEngine,
    /// The undo journal (the [`Store`]).
    pub journal: Arc<dyn Journal>,
    /// Settings (the same [`Store`]).
    pub settings: Arc<dyn Settings>,
    /// The activity log (the same [`Store`]).
    pub activity: Arc<dyn ActivityLog>,
    /// What building the core found.
    pub startup: StartupReport,
    gate: Mutex<()>,
}

impl fmt::Debug for AppCore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AppCore")
            .field("engine", &self.engine)
            .field("startup", &self.startup)
            .finish_non_exhaustive()
    }
}

impl AppCore {
    /// Assembles a core from parts. [`build_core`] is the normal way; this exists so tests can
    /// wire an engine over their own fakes (for example an activity log that fails).
    #[must_use]
    pub fn new(
        engine: TweakEngine,
        journal: Arc<dyn Journal>,
        settings: Arc<dyn Settings>,
        activity: Arc<dyn ActivityLog>,
        startup: StartupReport,
    ) -> Self {
        Self {
            engine,
            journal,
            settings,
            activity,
            startup,
            gate: Mutex::new(()),
        }
    }

    /// Serialises the operations that read or change tweak state, so two quick clicks (or a
    /// list that overlaps an apply) cannot interleave. Hold the guard for the whole call.
    pub fn exclusive(&self) -> MutexGuard<'_, ()> {
        // The guard protects no data, so a poisoned lock only means another call panicked.
        self.gate.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Builds the core: opens the one store at `store_path`, wires the executor and the tweak
/// engine (with the activity log attached) over `ports`, and calls `reconcile_pending` once.
///
/// `catalogue_toml` is the catalogue text; the app passes [`EMBEDDED_CATALOGUE`].
///
/// A corrupt database is moved aside by the store and reported in
/// [`StartupReport::recovery`] (and logged); it is never deleted. A failure to reconcile is
/// logged and kept in the report, it does not stop the app: the journal is untouched and the
/// entries stay pending for the next start.
pub fn build_core(
    ports: Ports,
    store_path: &Path,
    catalogue_toml: &str,
) -> Result<AppCore, InitError> {
    let catalogue = Arc::new(Catalogue::from_toml(catalogue_toml)?);
    let OpenOutcome { store, recovery } = Store::open(store_path)?;
    if let Some(recovery) = &recovery {
        log::warn!(
            "started with a new database because the old one was corrupt ({}); the old one was \
             kept at {}. Changes made before cannot be undone from Volt.",
            recovery.reason,
            recovery.backup_path.display()
        );
    }

    // One store, three ports.
    let store = Arc::new(store);
    let journal: Arc<dyn Journal> = store.clone();
    let settings: Arc<dyn Settings> = store.clone();
    let activity: Arc<dyn ActivityLog> = store;
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);

    let reconcile = Executor::new(ports.clone(), journal.clone(), clock.clone())
        .with_activity_log(activity.clone())
        .reconcile_pending();
    log_reconcile(&reconcile);

    let engine = TweakEngine::new(catalogue, ports, journal.clone(), clock)
        .with_activity_log(activity.clone());
    Ok(AppCore::new(
        engine,
        journal,
        settings,
        activity,
        StartupReport {
            store_path: store_path.to_path_buf(),
            recovery,
            reconcile,
        },
    ))
}

/// The ports of the real machine: the Windows adapters of `volt-win`, plus
/// [`Unsupported`](PortError::Unsupported) stand-ins for the two ports that have no adapter
/// yet (processes and memory).
#[must_use]
pub fn windows_ports() -> Ports {
    Ports {
        registry: Arc::new(WinRegistry::new()),
        file_system: Arc::new(WinFileSystem::new()),
        services: Arc::new(WinServices::new()),
        processes: Arc::new(UnsupportedProcesses),
        memory: Arc::new(UnsupportedMemory),
        commands: Arc::new(WinCommands::new()),
    }
}

/// Writes the outcome of the start-up reconciliation to the diagnostic log.
fn log_reconcile(result: &Result<ReconcileReport, PortError>) {
    match result {
        Err(error) => log::error!(
            "could not resolve interrupted changes: {}",
            volt_core::activity::error_class(error)
        ),
        Ok(report) if report.outcomes.is_empty() => {
            log::info!("no interrupted changes to resolve");
        }
        Ok(report) => {
            let count = |wanted: fn(&Resolution) -> bool| {
                report
                    .outcomes
                    .iter()
                    .filter(|outcome| wanted(&outcome.resolution))
                    .count()
            };
            let confirmed = count(|r| matches!(r, Resolution::ConfirmedApplied));
            let never = count(|r| matches!(r, Resolution::NeverApplied));
            let unknown = count(|r| matches!(r, Resolution::StateUnknown));
            let undetermined = count(|r| matches!(r, Resolution::Undetermined { .. }));
            let message = format!(
                "resolved {} interrupted change(s): {confirmed} confirmed applied, {never} \
                 never applied, {unknown} in an unknown state, {undetermined} left pending",
                report.outcomes.len()
            );
            if unknown + undetermined > 0 {
                log::warn!("{message}");
            } else {
                log::info!("{message}");
            }
        }
    }
}

/// Tauri managed state: the core, or the reason it could not be built.
///
/// The app still starts when the core is missing (the tray and the window must stay usable), and
/// every command then answers with the same error, which the UI shows.
#[derive(Debug)]
pub struct AppState {
    core: Result<Arc<AppCore>, String>,
}

impl AppState {
    /// State holding a built core.
    #[must_use]
    pub fn ready(core: AppCore) -> Self {
        Self {
            core: Ok(Arc::new(core)),
        }
    }

    /// State for a core that could not be built.
    #[must_use]
    pub fn failed(error: &InitError) -> Self {
        Self {
            core: Err(format!("Volt could not start its tweak engine: {error}")),
        }
    }

    /// The core, or the error every command returns while there is none.
    pub fn core(&self) -> Result<Arc<AppCore>, IpcError> {
        self.core
            .as_ref()
            .map(Arc::clone)
            .map_err(|message| IpcError::Other {
                message: message.clone(),
            })
    }
}

/// Builds the real core in the app data directory. Called once from the app's setup, before any
/// window exists. A failure is logged and kept in the returned state.
pub(crate) fn init(app: &AppHandle) -> AppState {
    match build_real_core(app) {
        Ok(core) => AppState::ready(core),
        Err(error) => {
            log::error!("the tweak engine could not start: {error}");
            AppState::failed(&error)
        }
    }
}

fn build_real_core(app: &AppHandle) -> Result<AppCore, InitError> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|error| InitError::DataDir(error.to_string()))?;
    std::fs::create_dir_all(&dir).map_err(|error| InitError::DataDir(error.to_string()))?;
    build_core(
        windows_ports(),
        &dir.join(STORE_FILE_NAME),
        EMBEDDED_CATALOGUE,
    )
}
