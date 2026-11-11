//! Installed apps: listing them and removing them one at a time.
//!
//! Two kinds of app are listed. Desktop (Win32) apps come from the Uninstall registry keys, read
//! through the [`Registry`] port. Packaged (Store) apps of the current user come from the
//! [`PackagedApps`] port. Listing only reads.
//!
//! Removing an app is deliberately not an `Operation`: what an uninstaller deletes cannot be put
//! back, so nothing goes through the undo journal. [`AppManager::uninstall`] starts the vendor's
//! own registered command through the [`Uninstaller`] port (interactive and unelevated: the
//! vendor's dialog and any administrator prompt are the vendor's) or, for a packaged app, asks the
//! package manager to remove it for the current user. It waits for that to finish, reads the list
//! again and reports whether the app is gone, and writes one activity record that says Volt
//! cannot undo it.
//!
//! Many uninstallers copy themselves to a temporary folder, start the copy and exit at once. The
//! [`Uninstaller`] port therefore waits for every process the uninstaller started, not only the
//! first, and says how far it got ([`Waited`]). When it could not follow the later processes (an
//! elevated uninstaller does not join the job of an unelevated launcher), the manager waits until
//! the app's registry entry disappears instead. Either wait ends early when the user asks
//! ([`AppManager::stop_waiting`]) and the fallback also gives up after a limit.
//!
//! Which entries are offered is decided here, not in the UI: entries marked as system
//! components, updates, frameworks, packages Windows will not remove and entries without a usable
//! uninstall command carry a [`HiddenReason`] (the UI hides them by default), and Volt itself is
//! never listed at all.

mod command;
#[cfg(any(test, feature = "fakes"))]
pub mod fakes;
#[cfg(test)]
mod tests;
mod win32;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, TryLockError};
use std::time::{Duration, Instant};

use thiserror::Error;

pub use command::{CommandRefusal, UninstallCommand};
pub use win32::AppLocation;

use crate::activity::{ActivityLog, ActivityOutcome, NewActivity, error_class};
use crate::error::PortError;
use crate::journal::Source;
use crate::phrase::{Phrase, class_phrase};
use crate::registry::Registry;
use crate::time::{Clock, Timestamp};

/// The activity label of removing an app.
pub const UNINSTALL_LABEL: &str = "apps.uninstall";

/// Where an app was listed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AppSource {
    /// Installed for every user, in the 64-bit registry view.
    Machine64,
    /// Installed for every user, in the 32-bit registry view.
    Machine32,
    /// Installed for the current user only (the current-user registry key).
    CurrentUser,
    /// A packaged (Store) app of the current user.
    Packaged,
}

impl AppSource {
    fn id_part(self) -> &'static str {
        match self {
            Self::Machine64 => "m64",
            Self::Machine32 => "m32",
            Self::CurrentUser => "user",
            Self::Packaged => "pkg",
        }
    }
}

/// Why an entry is hidden by default. The UI shows these only behind "Show system components".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HiddenReason {
    /// The entry is marked as a system component.
    SystemComponent,
    /// An update or a patch of another product.
    Update,
    /// A framework or runtime package other apps depend on.
    Framework,
    /// Windows or the entry itself says it cannot be removed.
    NotRemovable,
    /// There is no usable uninstall command (none registered, or one Volt refuses).
    NoUninstallCommand,
}

/// A calendar date, such as the day an app was installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct AppDate {
    /// Year, four digits.
    pub year: u16,
    /// Month, 1 to 12.
    pub month: u8,
    /// Day of the month, 1 to 31.
    pub day: u8,
}

impl AppDate {
    /// Reads the `YYYYMMDD` text Windows Installer writes (`20240131`). Anything else, and an
    /// impossible date, is `None`.
    #[must_use]
    pub fn parse_compact(text: &str) -> Option<Self> {
        if text.len() != 8 || !text.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        Self::new(
            text.get(0..4)?.parse().ok()?,
            text.get(4..6)?.parse().ok()?,
            text.get(6..8)?.parse().ok()?,
        )
    }

    /// The UTC day of a point in time.
    #[must_use]
    pub fn of(time: Timestamp) -> Option<Self> {
        let iso = time.to_iso8601();
        Self::new(
            iso.get(0..4)?.parse().ok()?,
            iso.get(5..7)?.parse().ok()?,
            iso.get(8..10)?.parse().ok()?,
        )
    }

    fn new(year: u16, month: u8, day: u8) -> Option<Self> {
        ((1980..=2200).contains(&year) && (1..=12).contains(&month) && (1..=31).contains(&day))
            .then_some(Self { year, month, day })
    }
}

impl std::fmt::Display for AppDate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

/// How an entry is removed.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Removal {
    /// The vendor's registered uninstall command.
    Command(UninstallCommand),
    /// A packaged app, by its package full name.
    Package(String),
    /// Nothing usable.
    None,
}

/// One installed app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledApp {
    /// Stable identity of the entry (`win32:<place>:<registry key name>` or `pkg:<full name>`).
    /// Pass it to [`AppManager::uninstall`]; it is never a command.
    pub id: String,
    /// Where it was listed.
    pub source: AppSource,
    /// The name shown to the user.
    pub name: String,
    /// Who published it, when known.
    pub publisher: Option<String>,
    /// Its version, when known.
    pub version: Option<String>,
    /// Its size on disk in bytes, when the entry says (packaged apps do not).
    pub size_bytes: Option<u64>,
    /// The day it was installed, when known.
    pub installed: Option<AppDate>,
    /// Why it is hidden by default; `None` for an ordinary app.
    pub hidden: Option<HiddenReason>,
    removal: Removal,
}

impl InstalledApp {
    /// Whether Volt can remove it: it has a usable command or package, is not a framework and is
    /// not marked as non-removable. A system component or an update with a command can be
    /// removed (the UI only shows it after the user asked for system components).
    #[must_use]
    pub fn uninstallable(&self) -> bool {
        !matches!(
            self.hidden,
            Some(
                HiddenReason::NoUninstallCommand
                    | HiddenReason::NotRemovable
                    | HiddenReason::Framework
            )
        ) && !matches!(self.removal, Removal::None)
    }
}

/// A source that could not be read completely.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppProblem {
    /// Which source.
    pub source: AppSource,
    /// The whole source could not be read.
    pub unreadable: bool,
    /// Entries of the source that could not be read and are missing from the list.
    pub skipped: u32,
}

/// Every app found, and what could not be read.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AppListing {
    /// The apps, sorted by name ignoring case.
    pub apps: Vec<InstalledApp>,
    /// Sources that could not be read completely. The rest of the list is valid.
    pub problems: Vec<AppProblem>,
}

/// What the package manager says about one packaged app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackagedApp {
    /// The package full name, which removing it takes.
    pub full_name: String,
    /// The name shown to the user.
    pub name: String,
    /// Who published it, when known.
    pub publisher: Option<String>,
    /// Its version, when known.
    pub version: Option<String>,
    /// When it was installed, when known.
    pub installed: Option<Timestamp>,
    /// Its size in bytes, when known. Windows has no cheap way to report one for a package (only
    /// walking its folder, which the listing never does), so the adapter leaves it empty.
    pub size_bytes: Option<u64>,
    /// A framework other packages depend on.
    pub framework: bool,
    /// A resource package (language or scale assets), which is not an app.
    pub resource: bool,
    /// Signed by Windows itself: part of the system, not removable.
    pub system: bool,
}

/// Port: the packaged (Store) apps of the current user.
pub trait PackagedApps: Send + Sync {
    /// The packages installed for the current user. Read-only.
    fn list(&self) -> Result<Vec<PackagedApp>, PortError>;

    /// Removes a package for the current user (never for other users). Waits until Windows is
    /// done.
    fn remove(&self, full_name: &str) -> Result<(), PortError>;
}

/// A request to stop waiting for the uninstaller that is running. Cheap to clone; every clone is
/// the same request.
#[derive(Debug, Clone, Default)]
pub struct StopWaiting(Arc<AtomicBool>);

impl StopWaiting {
    /// A request that was not made yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Asks the wait to end. The uninstaller itself is never touched.
    pub fn request(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    /// Whether the wait was asked to end.
    #[must_use]
    pub fn is_requested(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    fn clear(&self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// How far the wait for an uninstaller got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Waited {
    /// The uninstaller and every process it started have exited.
    Everything,
    /// The first process exited, but the processes it started could not be followed (for example
    /// an elevated uninstaller started from an unelevated launcher). Something may still run.
    FirstProcess,
    /// The wait ended because the user asked it to. Something may still run.
    Stopped,
}

/// What the wait for an uninstaller found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UninstallerExit {
    /// The exit code of the process Volt started, when it exited and Windows reported one.
    pub exit_code: Option<i32>,
    /// How far the wait got.
    pub waited: Waited,
}

impl UninstallerExit {
    /// The uninstaller and everything it started have exited.
    #[must_use]
    pub const fn complete(exit_code: Option<i32>) -> Self {
        Self {
            exit_code,
            waited: Waited::Everything,
        }
    }
}

/// Port: starting a vendor's uninstaller.
pub trait Uninstaller: Send + Sync {
    /// Starts `command` exactly as registered, interactively and without elevating, and waits
    /// until the process and every process it started have exited, or until `stop` is requested.
    /// A program that cannot be started is an error (nothing ran).
    fn run(
        &self,
        command: &UninstallCommand,
        stop: &StopWaiting,
    ) -> Result<UninstallerExit, PortError>;
}

/// Where Volt itself is installed, so it is never offered for removal.
#[derive(Debug, Clone, Default)]
pub struct OwnInstall {
    folder: Option<String>,
}

impl OwnInstall {
    /// Volt runs from `folder` (the folder of its executable).
    #[must_use]
    pub fn new(folder: Option<String>) -> Self {
        Self {
            folder: folder.map(|folder| normalise(&folder)),
        }
    }

    /// Whether an entry is Volt: it is named Volt, or the folder of its uninstaller or its install
    /// location is Volt's folder or inside it.
    fn is_volt<'a>(&self, name: &str, folders: impl IntoIterator<Item = &'a str>) -> bool {
        if name.trim().eq_ignore_ascii_case("volt") {
            return true;
        }
        let Some(own) = self.folder.as_deref().filter(|own| !own.is_empty()) else {
            return false;
        };
        folders.into_iter().any(|folder| {
            let folder = normalise(folder);
            folder == own
                || folder
                    .strip_prefix(own)
                    .is_some_and(|rest| rest.starts_with('\\'))
        })
    }
}

fn normalise(path: &str) -> String {
    path.trim()
        .trim_matches('"')
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase()
}

/// How removing one app ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum UninstallOutcome {
    /// The uninstaller finished and the app is no longer listed.
    Removed,
    /// The uninstaller finished (or was closed) and the app is still listed.
    StillInstalled,
    /// Nothing ran: the uninstaller could not be started or Windows refused the removal. Carries
    /// the class of the failure (never a message).
    Failed(&'static str),
    /// The uninstaller finished, but the list could not be read again to check.
    Unknown,
}

/// What removing one app did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UninstallReport {
    /// The entry that was removed.
    pub id: String,
    /// Its name.
    pub name: String,
    /// Where it was listed.
    pub source: AppSource,
    /// How it ended.
    pub outcome: UninstallOutcome,
    /// The exit code of the vendor's uninstaller, when it ran and Windows reported one.
    pub exit_code: Option<i32>,
    /// The list as it is after the uninstaller finished.
    pub listing: AppListing,
    /// Whether the activity record could not be written (the removal itself is unaffected).
    pub activity_write_failed: bool,
}

/// Why nothing was started. Nothing was changed.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum UninstallError {
    /// The entry is not in the list (it may have just been removed).
    #[error("that app is not installed any more")]
    NotFound,
    /// The entry cannot be removed by Volt: it has no usable uninstall command (none registered,
    /// or one [`UninstallCommand::parse`] refuses), or it is a framework or non-removable.
    #[error("this app cannot be removed from Volt")]
    NotUninstallable,
    /// Another removal is running.
    #[error("another app is being removed; wait for it to finish")]
    Busy,
    /// The entry is a system component or an update, which stay hidden until the user asked for
    /// them.
    #[error("this entry is hidden by default; show system components first")]
    HiddenByDefault,
}

/// Lists installed apps and removes them, over the ports.
pub struct AppManager {
    registry: Arc<dyn Registry>,
    uninstaller: Arc<dyn Uninstaller>,
    packages: Option<Arc<dyn PackagedApps>>,
    locations: Vec<AppLocation>,
    own: OwnInstall,
    clock: Arc<dyn Clock>,
    activity: Option<Arc<dyn ActivityLog>>,
    running: Mutex<()>,
    stop: StopWaiting,
    poll: Duration,
    limit: Duration,
}

/// How often the registry is read while waiting for an uninstaller that could not be followed.
const DEFAULT_POLL: Duration = Duration::from_secs(2);
/// The longest Volt waits for such an uninstaller before it reports what it sees.
const DEFAULT_LIMIT: Duration = Duration::from_secs(20 * 60);
/// A wait sleeps in slices this long, so a request to stop is noticed quickly.
const SLICE: Duration = Duration::from_millis(100);

impl std::fmt::Debug for AppManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppManager").finish_non_exhaustive()
    }
}

impl AppManager {
    /// A manager reading the three Windows locations of `registry` and starting uninstallers
    /// through `uninstaller`. There are no packaged apps and no activity log until they are added.
    #[must_use]
    pub fn new(
        registry: Arc<dyn Registry>,
        uninstaller: Arc<dyn Uninstaller>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            registry,
            uninstaller,
            packages: None,
            locations: AppLocation::windows(),
            own: OwnInstall::default(),
            clock,
            activity: None,
            running: Mutex::new(()),
            stop: StopWaiting::new(),
            poll: DEFAULT_POLL,
            limit: DEFAULT_LIMIT,
        }
    }

    /// Changes how an uninstaller that could not be followed is waited for: the registry is read
    /// every `poll` and the wait gives up after `limit`.
    #[must_use]
    pub fn with_untracked_wait(mut self, poll: Duration, limit: Duration) -> Self {
        self.poll = poll;
        self.limit = limit;
        self
    }

    /// Ends the wait for the uninstaller that is running, if any. The uninstaller keeps running;
    /// the removal is then reported from what the list shows.
    pub fn stop_waiting(&self) {
        self.stop.request();
    }

    /// Also lists (and removes) the packaged apps of the current user.
    #[must_use]
    pub fn with_packages(mut self, packages: Arc<dyn PackagedApps>) -> Self {
        self.packages = Some(packages);
        self
    }

    /// Reads the registry at `locations` instead of the Windows ones (a test points them at a
    /// throwaway key).
    #[must_use]
    pub fn with_locations(mut self, locations: Vec<AppLocation>) -> Self {
        self.locations = locations;
        self
    }

    /// Never lists Volt: the entry named Volt and anything that lives in `folder`.
    #[must_use]
    pub fn with_own_install(mut self, own: OwnInstall) -> Self {
        self.own = own;
        self
    }

    /// Writes one activity record per removal to `log`.
    #[must_use]
    pub fn with_activity_log(mut self, log: Arc<dyn ActivityLog>) -> Self {
        self.activity = Some(log);
        self
    }

    /// Reads every installed app. Read-only. A source that cannot be read is a problem in the
    /// listing, never a failure of the whole list.
    #[must_use]
    pub fn list(&self) -> AppListing {
        let mut apps = Vec::new();
        let mut problems = Vec::new();
        for location in &self.locations {
            match win32::read_location(self.registry.as_ref(), location, &self.own) {
                Ok(read) => {
                    apps.extend(read.apps);
                    if read.skipped > 0 {
                        problems.push(AppProblem {
                            source: location.source,
                            unreadable: false,
                            skipped: read.skipped,
                        });
                    }
                }
                Err(error) => {
                    log::warn!(
                        "could not read the installed apps of {:?} ({})",
                        location.source,
                        error_class(&error)
                    );
                    problems.push(AppProblem {
                        source: location.source,
                        unreadable: true,
                        skipped: 0,
                    });
                }
            }
        }
        if let Some(packages) = &self.packages {
            match packages.list() {
                Ok(found) => apps.extend(found.iter().filter_map(packaged_app)),
                Err(error) => {
                    log::warn!("could not read the packaged apps ({})", error_class(&error));
                    problems.push(AppProblem {
                        source: AppSource::Packaged,
                        unreadable: true,
                        skipped: 0,
                    });
                }
            }
        }
        apps.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then_with(|| a.id.cmp(&b.id))
        });
        AppListing { apps, problems }
    }

    /// Removes the app `id` (an [`InstalledApp::id`] of a fresh reading), waits until its
    /// uninstaller exits, reads the list again and records the result.
    ///
    /// The command to start is always the one in the fresh reading, never one passed in. One
    /// removal runs at a time: a second call while one runs is [`UninstallError::Busy`]. A system
    /// component or an update is refused ([`UninstallError::HiddenByDefault`]); see
    /// [`AppManager::uninstall_including_system`].
    pub fn uninstall(&self, id: &str, source: Source) -> Result<UninstallReport, UninstallError> {
        self.uninstall_with(id, source, false)
    }

    /// Like [`AppManager::uninstall`], but also removes a system component or an update. The
    /// caller passes this only when the user turned on "Show system components".
    pub fn uninstall_including_system(
        &self,
        id: &str,
        source: Source,
    ) -> Result<UninstallReport, UninstallError> {
        self.uninstall_with(id, source, true)
    }

    fn uninstall_with(
        &self,
        id: &str,
        source: Source,
        include_system: bool,
    ) -> Result<UninstallReport, UninstallError> {
        let _running = match self.running.try_lock() {
            Ok(guard) => guard,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => return Err(UninstallError::Busy),
        };
        let before = self.list();
        let app = before
            .apps
            .iter()
            .find(|app| app.id == id)
            .ok_or(UninstallError::NotFound)?;
        if !app.uninstallable() {
            return Err(UninstallError::NotUninstallable);
        }
        if !include_system
            && matches!(
                app.hidden,
                Some(HiddenReason::SystemComponent | HiddenReason::Update)
            )
        {
            return Err(UninstallError::HiddenByDefault);
        }
        self.stop.clear();
        let started = match &app.removal {
            Removal::Command(command) => self
                .uninstaller
                .run(command, &self.stop)
                .map(|exit| self.finish_wait(id, exit)),
            Removal::Package(full_name) => self
                .packages
                .as_ref()
                .ok_or(UninstallError::NotUninstallable)?
                .remove(full_name)
                .map(|()| None),
            Removal::None => return Err(UninstallError::NotUninstallable),
        };
        let (outcome, exit_code, listing) = match started {
            Err(error) => (
                UninstallOutcome::Failed(error_class(&error)),
                None,
                before.clone(),
            ),
            Ok(exit_code) => {
                let after = self.list();
                let outcome = if after.apps.iter().any(|candidate| candidate.id == id) {
                    UninstallOutcome::StillInstalled
                } else if after
                    .problems
                    .iter()
                    .any(|problem| problem.source == app.source && problem.unreadable)
                {
                    UninstallOutcome::Unknown
                } else {
                    UninstallOutcome::Removed
                };
                (outcome, exit_code, after)
            }
        };
        let mut report = UninstallReport {
            id: app.id.clone(),
            name: app.name.clone(),
            source: app.source,
            outcome,
            exit_code,
            listing,
            activity_write_failed: false,
        };
        report.activity_write_failed = !self.record(&report, source);
        Ok(report)
    }

    /// For an uninstaller that could not be followed past its first process: waits until the
    /// app's entry is gone from the list, the user stops waiting or the limit passes. Returns the
    /// exit code of the first process.
    fn finish_wait(&self, id: &str, exit: UninstallerExit) -> Option<i32> {
        if exit.waited == Waited::FirstProcess {
            let started = Instant::now();
            while !self.stop.is_requested()
                && started.elapsed() < self.limit
                && self.list().apps.iter().any(|app| app.id == id)
            {
                let mut slept = Duration::ZERO;
                while slept < self.poll && !self.stop.is_requested() {
                    let slice = SLICE.min(self.poll - slept);
                    std::thread::sleep(slice);
                    slept += slice;
                }
            }
        }
        exit.exit_code
    }

    /// Writes the activity record. Returns whether it was written (or there is no log).
    fn record(&self, report: &UninstallReport, source: Source) -> bool {
        let Some(log) = &self.activity else {
            return true;
        };
        let name = |code: &str| Phrase::new(code).text("name", report.name.clone());
        let (outcome, phrase) = match report.outcome {
            UninstallOutcome::Removed => {
                (ActivityOutcome::Applied, name("summary.uninstall.removed"))
            }
            UninstallOutcome::StillInstalled => (
                ActivityOutcome::Failed,
                name("summary.uninstall.stillInstalled"),
            ),
            UninstallOutcome::Unknown => {
                (ActivityOutcome::Failed, name("summary.uninstall.unknown"))
            }
            UninstallOutcome::Failed(class) => (
                ActivityOutcome::Failed,
                name("summary.uninstall.failed").phrase("reason", class_phrase(class)),
            ),
        };
        let activity = NewActivity::said(
            self.clock.now(),
            source,
            outcome,
            phrase,
            Some(UNINSTALL_LABEL.to_owned()),
        );
        match log.record(activity) {
            Ok(_) => true,
            Err(error) => {
                log::warn!(
                    "could not write the activity record for removing an app ({})",
                    error_class(&error)
                );
                false
            }
        }
    }
}

/// The entry of a package, or `None` for a resource package (not an app).
fn packaged_app(package: &PackagedApp) -> Option<InstalledApp> {
    if package.resource || package.full_name.trim().is_empty() {
        return None;
    }
    let hidden = if package.system {
        Some(HiddenReason::NotRemovable)
    } else if package.framework {
        Some(HiddenReason::Framework)
    } else {
        None
    };
    Some(InstalledApp {
        id: format!("{}:{}", AppSource::Packaged.id_part(), package.full_name),
        source: AppSource::Packaged,
        name: package.name.clone(),
        publisher: package
            .publisher
            .clone()
            .filter(|text| !text.trim().is_empty()),
        version: package
            .version
            .clone()
            .filter(|text| !text.trim().is_empty()),
        size_bytes: package.size_bytes,
        installed: package.installed.and_then(AppDate::of),
        hidden,
        removal: Removal::Package(package.full_name.clone()),
    })
}
