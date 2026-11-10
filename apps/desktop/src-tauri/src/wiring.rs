//! The composition root: where the adapters are wired into the core.
//!
//! [`build_core`] is the one place that knows every crate. It opens the [`Store`] once (the
//! undo journal, the settings and the activity log are the same database), builds the executor
//! and the tweak engine over the given [`Ports`], builds the clean engine (`CleaningSetup`) and
//! resolves entries a crash left pending.
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
    ActivityLog, Catalogue, CatalogueError, CleanEngine, CleaningRules, Clock, Executor,
    FilePublisher, Journal, KnownFolders, MemoryCleaner, PortError, Ports, ReconcileReport,
    RecycleBinManager, Resolution, RulesError, Settings, StartupManager, StartupSources,
    SystemClock, TweakEngine,
};
use volt_store::{CorruptionRecovery, OpenOutcome, Store, StoreError};
use volt_win::{
    WinCommands, WinFilePublisher, WinFileSystem, WinKnownFolders, WinMemory, WinProcesses,
    WinRecycleBin, WinRegistry, WinServices,
};

use crate::IpcError;
use crate::catalogue::{EMBEDDED_CATALOGUE, EMBEDDED_CLEANING_RULES};
use crate::cleaner::CleanSession;

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
    /// The cleaning rules text is invalid. The embedded file is tested, so this is a build bug.
    CleaningRules(RulesError),
    /// The app data directory could not be found or created.
    DataDir(String),
}

impl fmt::Display for InitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(error) => write!(f, "{error}"),
            Self::Catalogue(error) => write!(f, "{error}"),
            Self::CleaningRules(error) => write!(f, "{error}"),
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

impl From<RulesError> for InitError {
    fn from(error: RulesError) -> Self {
        Self::CleaningRules(error)
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
    startup_apps: Option<StartupManager>,
    gate: Mutex<()>,
    clean: Option<CleanEngine>,
    clean_session: CleanSession,
    recycle_bin: Option<RecycleBinManager>,
    memory_cleaner: Option<MemoryCleaner>,
}

impl fmt::Debug for AppCore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AppCore")
            .field("engine", &self.engine)
            .field("clean", &self.clean)
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
            startup_apps: None,
            gate: Mutex::new(()),
            clean: None,
            clean_session: CleanSession::default(),
            recycle_bin: None,
            memory_cleaner: None,
        }
    }

    /// Attaches the clean engine. [`build_core`] always does; a core assembled by hand with
    /// [`AppCore::new`] has none until this is called.
    #[must_use]
    pub fn with_clean_engine(mut self, engine: CleanEngine) -> Self {
        self.clean = Some(engine);
        self
    }

    /// The clean engine: scans and cleans with the embedded rules, writes the activity log and
    /// reads the settings. `None` only for a core assembled by hand without one.
    #[must_use]
    pub fn clean_engine(&self) -> Option<&CleanEngine> {
        self.clean.as_ref()
    }

    /// Attaches the Recycle Bin manager. The real app does in [`init`]; [`build_core`] never
    /// does, so a test cannot reach the real Recycle Bin by accident.
    #[must_use]
    pub fn with_recycle_bin(mut self, manager: RecycleBinManager) -> Self {
        self.recycle_bin = Some(manager);
        self
    }

    /// The Recycle Bin manager, or `None` for a core assembled without one.
    #[must_use]
    pub fn recycle_bin(&self) -> Option<&RecycleBinManager> {
        self.recycle_bin.as_ref()
    }

    /// What the cleaner keeps between a scan and a clean: the latest plan and the scan flags.
    #[must_use]
    pub fn clean_session(&self) -> &CleanSession {
        &self.clean_session
    }

    /// The same core with the memory cleaner attached. [`build_core`] always does; a core
    /// assembled by hand has none until this is called.
    #[must_use]
    pub fn with_memory_cleaner(mut self, cleaner: MemoryCleaner) -> Self {
        self.memory_cleaner = Some(cleaner);
        self
    }

    /// The memory cleaner, or the error the memory commands return without one.
    pub fn memory_cleaner(&self) -> Result<&MemoryCleaner, IpcError> {
        self.memory_cleaner
            .as_ref()
            .ok_or_else(|| IpcError::Unsupported {
                message: "memory cleaning is not available in this build".to_owned(),
            })
    }

    /// The same core with the startup apps manager attached. [`build_core_with`] does this;
    /// a core built without it answers the startup commands with `unsupported`.
    #[must_use]
    pub fn with_startup_apps(mut self, manager: StartupManager) -> Self {
        self.startup_apps = Some(manager);
        self
    }

    /// The startup apps manager, or the error the startup commands return without one.
    pub fn startup_apps(&self) -> Result<&StartupManager, IpcError> {
        self.startup_apps
            .as_ref()
            .ok_or_else(|| IpcError::Unsupported {
                message: "the startup apps manager is not available in this build".to_owned(),
            })
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
    build_core_with_cleaning(ports, store_path, catalogue_toml, CleaningSetup::windows())
}

/// What the clean engine is built from, besides the ports, the clock and the store: the rules
/// text and the resolver of the rules' named locations.
pub struct CleaningSetup<'a> {
    /// The cleaning rules, as TOML text. The app passes [`EMBEDDED_CLEANING_RULES`].
    pub rules_toml: &'a str,
    /// Resolves the rules' named locations to folders. The app passes [`WinKnownFolders`]; a
    /// test passes a fake or a double that points at a temp directory.
    pub folders: Arc<dyn KnownFolders>,
}

impl fmt::Debug for CleaningSetup<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CleaningSetup")
            .field("rules_bytes", &self.rules_toml.len())
            .finish_non_exhaustive()
    }
}

impl CleaningSetup<'static> {
    /// The real setup: the embedded rules and the Windows known folders.
    #[must_use]
    pub fn windows() -> Self {
        Self {
            rules_toml: EMBEDDED_CLEANING_RULES,
            folders: Arc::new(WinKnownFolders::new()),
        }
    }
}

/// What the startup apps manager reads and where: the registry keys and folders, and the port
/// that tells who published a program. The app uses [`StartupWiring::windows`]; tests point the
/// sources at a throwaway key and temp folders ([`StartupSources::sandboxed_in_hkcu`]).
pub struct StartupWiring {
    /// Where entries are read from and approved.
    pub sources: StartupSources,
    /// Reads the publisher of a program.
    pub publisher: Arc<dyn FilePublisher>,
}

impl fmt::Debug for StartupWiring {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StartupWiring")
            .field("sources", &self.sources)
            .finish_non_exhaustive()
    }
}

impl StartupWiring {
    /// The real machine: the Run keys and Startup folders of Windows and the version-information
    /// reader.
    #[must_use]
    pub fn windows() -> Self {
        Self {
            sources: StartupSources::windows(&WinKnownFolders::new()),
            publisher: Arc::new(WinFilePublisher::new()),
        }
    }
}

/// [`build_core`] with the clean engine's rules and known folders given. The clean engine is
/// built from `ports.file_system`, the system clock and the one store (as activity log and as
/// settings, for the minimum age of the temp rules).
///
/// Invalid cleaning rules are [`InitError::CleaningRules`], like an invalid tweak catalogue: the
/// text is checked before the database is opened.
pub fn build_core_with_cleaning(
    ports: Ports,
    store_path: &Path,
    catalogue_toml: &str,
    cleaning: CleaningSetup<'_>,
) -> Result<AppCore, InitError> {
    build_core_inner(ports, store_path, catalogue_toml, cleaning, None)
}

/// [`build_core`] plus the startup apps manager, which reads and approves entries through the
/// registry and file system of `ports` at the places `startup` names.
pub fn build_core_with(
    ports: Ports,
    store_path: &Path,
    catalogue_toml: &str,
    startup: StartupWiring,
) -> Result<AppCore, InitError> {
    build_core_inner(
        ports,
        store_path,
        catalogue_toml,
        CleaningSetup::windows(),
        Some(startup),
    )
}

fn build_core_inner(
    ports: Ports,
    store_path: &Path,
    catalogue_toml: &str,
    cleaning: CleaningSetup<'_>,
    startup: Option<StartupWiring>,
) -> Result<AppCore, InitError> {
    let catalogue = Arc::new(Catalogue::from_toml(catalogue_toml)?);
    let rules = CleaningRules::from_toml(cleaning.rules_toml)?;
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

    let clean = CleanEngine::new(
        rules,
        cleaning.folders,
        ports.file_system.clone(),
        clock.clone(),
    )
    .with_processes(ports.processes.clone())
    .with_activity_log(activity.clone())
    .with_settings(settings.clone());

    let memory_cleaner =
        MemoryCleaner::new(ports.memory.clone(), clock.clone()).with_activity_log(activity.clone());

    let (registry, file_system) = (ports.registry.clone(), ports.file_system.clone());
    let startup_clock = clock.clone();
    let engine = TweakEngine::new(catalogue, ports, journal.clone(), clock)
        .with_activity_log(activity.clone());
    let startup_apps = startup.map(|wiring| {
        StartupManager::new(
            registry,
            file_system,
            wiring.publisher,
            // A handle on the engine's executor: the same journal, activity log and
            // write-failure counter, so every startup change is recorded like a tweak's.
            engine.executor().clone(),
            startup_clock,
            wiring.sources,
        )
    });
    let core = AppCore::new(
        engine,
        journal,
        settings,
        activity,
        StartupReport {
            store_path: store_path.to_path_buf(),
            recovery,
            reconcile,
        },
    )
    .with_clean_engine(clean)
    .with_memory_cleaner(memory_cleaner);
    Ok(match startup_apps {
        Some(manager) => core.with_startup_apps(manager),
        None => core,
    })
}

/// The ports of the real machine: the Windows adapters of `volt-win`, plus
/// an [`Unsupported`](PortError::Unsupported) stand-in for the processes port, which has no
/// adapter yet.
#[must_use]
pub fn windows_ports() -> Ports {
    Ports {
        registry: Arc::new(WinRegistry::new()),
        file_system: Arc::new(WinFileSystem::new()),
        services: Arc::new(WinServices::new()),
        processes: Arc::new(WinProcesses::new()),
        memory: Arc::new(WinMemory::new()),
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
    let core = build_core_with(
        windows_ports(),
        &dir.join(STORE_FILE_NAME),
        EMBEDDED_CATALOGUE,
        StartupWiring::windows(),
    )?;
    // The real Recycle Bin is attached here only: the one place that runs against a real machine.
    let bin = RecycleBinManager::new(Arc::new(WinRecycleBin::new()), Arc::new(SystemClock))
        .with_activity_log(core.activity.clone());
    Ok(core.with_recycle_bin(bin))
}
