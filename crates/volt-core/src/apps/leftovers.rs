//! What an uninstaller left behind, found from evidence recorded before the uninstall.
//!
//! Finding leftovers by matching names afterwards is risky, so Volt reads the app's registry entry
//! (the install location, the folder of its uninstaller and of its icon, the publisher and product
//! names and any registry key the entry names) while the app is still installed, and keeps that as
//! a [`Footprint`]. After a successful uninstall [`LeftoverCleaner::find`] looks only at the places
//! the footprint points to, and gives every find a [`Confidence`] and the evidence in words.
//!
//! # Confidence
//!
//! - **High**: the recorded install folder, and a registry key the uninstall entry named. Only
//!   these are selected by default.
//! - **Medium**: the folder the uninstaller or the icon came from, and a per-user data folder or
//!   registry key named exactly like the product, or like the publisher and the product.
//! - **Low**: a folder or key named exactly like the publisher, and anything that is shared with
//!   another installed app from the same publisher (never high).
//!
//! # Never offered
//!
//! Nothing under the Windows folder; the roots of Program Files, `ProgramData`, the user profile
//! and `AppData` themselves (and anything above them); anything another installed app's entry
//! points at, or that holds it; Volt's own folder; anything that is, or sits behind, a link or
//! junction; registry keys outside `Software\<vendor>`, or of Microsoft's, Windows' or Volt's.
//! Machine-wide registry keys are shown, but locked: removing them needs the administrator helper.
//!
//! # Removing
//!
//! A folder goes to the Recycle Bin ([`FileSystem::move_to_trash`]) after looking at it again. A
//! per-user registry key is deleted through the executor, one key at a time from the deepest up, so
//! the journal holds what each key contained and the Undo page can put it back; a key with more
//! than [`MAX_KEYS`] subkeys, or more data than Volt records, is shown but locked. One activity
//! record lists what was removed.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use thiserror::Error;

use super::{InstalledApp, OwnInstall};
use crate::activity::{ActivityLog, ActivityOutcome, NewActivity, error_class};
use crate::clean::{KnownFolders, Location, starts_with_ci};
use crate::error::{ExecError, PlanError, PortError};
use crate::executor::{Executor, ItemStatus};
use crate::journal::{BatchId, Source};
use crate::label::{Evidence, Risk};
use crate::operation::Operation;
use crate::phrase::{Phrase, class_phrase};
use crate::plan::{OperationRequest, Plan};
use crate::ports::{EntryKind, FileSystem};
use crate::registry::{Registry, RegistryHive, RegistryKeyRef, RegistryView, values_recordable};
use crate::time::Clock;

/// The activity and journal label of removing leftovers.
pub const LEFTOVERS_LABEL: &str = "apps.leftovers";

/// The most registry keys (the key and everything below it) one find may hold. Every key is one
/// journal entry, so a bigger tree is left to the user.
pub const MAX_KEYS: usize = 60;

/// The deepest registry tree Volt reads below a key.
const MAX_DEPTH: usize = 8;

/// The most file system entries Volt looks at to size one folder.
const MAX_ENTRIES: usize = 100_000;

/// The most removed items an activity record names; the rest is counted.
const MAX_LISTED: usize = 8;

/// The shortest product or publisher name that is looked for in `AppData` and the registry.
const MIN_NAME: usize = 3;

/// Registry keys directly below `Software` that are never offered, whatever evidence points there.
const PROTECTED_VENDORS: [&str; 9] = [
    "microsoft",
    "classes",
    "policies",
    "windows",
    "clients",
    "registeredapplications",
    "volt",
    "wow6432node",
    "google",
];

/// Where a recorded folder came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FolderOrigin {
    /// The entry's `InstallLocation`.
    InstallLocation,
    /// The folder of the program the uninstall command starts.
    Uninstaller,
    /// The folder of the entry's `DisplayIcon`.
    Icon,
}

/// A folder the uninstall entry recorded, as written (it may start with `%VAR%`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RecordedFolder {
    pub(super) raw: String,
    pub(super) origin: FolderOrigin,
}

/// Everything the uninstall entry of an app said about where the app lives, read before the
/// uninstall.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Footprint {
    pub(super) name: String,
    pub(super) publisher: Option<String>,
    pub(super) folders: Vec<RecordedFolder>,
    /// Registry keys the entry's values name, as `HKCU\...` or `HKLM\...` paths.
    pub(super) keys: Vec<RegistryKeyRef>,
}

/// How sure Volt is that something belongs to the removed app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Confidence {
    /// Shared, or only the name fits.
    Low,
    /// The names fit exactly, or an indirect record points there.
    Medium,
    /// A record made before the uninstall points exactly there.
    High,
}

/// What a find is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LeftoverKind {
    /// A folder, moved to the Recycle Bin.
    Folder,
    /// A registry key, deleted with a journal record.
    RegistryKey,
}

/// Why a find cannot be removed from here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LeftoverLock {
    /// A machine-wide key: changing it needs the administrator helper.
    NeedsAdministrator,
    /// The key holds more than Volt can save for undo, so it is not deleted.
    CannotRecord,
}

impl LeftoverLock {
    /// Why, as a sentence for the UI.
    #[must_use]
    pub fn phrase(self) -> Phrase {
        Phrase::new(match self {
            Self::NeedsAdministrator => "leftover.lock.administrator",
            Self::CannotRecord => "leftover.lock.cannotRecord",
        })
    }
}

/// One folder or registry key the uninstaller seems to have left.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Leftover {
    /// Stable identity within the app; pass it to [`AppManager::remove_leftovers`](super::AppManager::remove_leftovers).
    pub id: String,
    /// Folder or registry key.
    pub kind: LeftoverKind,
    /// The path (`C:\...`) or key (`HKCU\...`).
    pub location: String,
    /// How sure Volt is.
    pub confidence: Confidence,
    /// Why Volt thinks so, one sentence per piece of evidence.
    pub evidence: Vec<Phrase>,
    /// A folder's size in bytes (0 for a key).
    pub size_bytes: u64,
    /// The files of a folder, or the keys of a registry tree.
    pub items: u64,
    /// Why it cannot be removed from here, if so.
    pub locked: Option<LeftoverLock>,
}

impl Leftover {
    /// Whether the UI selects it at first: only high confidence, and only when it can be removed.
    #[must_use]
    pub fn selected_by_default(&self) -> bool {
        self.confidence == Confidence::High && self.locked.is_none()
    }
}

/// What the scan for leftovers found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeftoverScan {
    /// The app the scan is for.
    pub app_name: String,
    /// The finds, the most certain first.
    pub items: Vec<Leftover>,
    /// `true` when something could not be read, or a size is only what was counted before Volt
    /// stopped looking.
    pub incomplete: bool,
}

/// Why one chosen find was not removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LeftoverSkip {
    /// Not one of the finds of a fresh scan.
    NotOffered,
    /// A locked find.
    Locked,
    /// Already gone.
    Gone,
    /// Not what the scan saw any more.
    Changed,
    /// It is, or sits behind, a link.
    Link,
    /// Volt or Windows refused it.
    Refused,
    /// Failed, with the class of the failure.
    Failed(&'static str),
}

impl LeftoverSkip {
    /// Why, as a sentence for the UI.
    #[must_use]
    pub fn phrase(self) -> Phrase {
        match self {
            Self::NotOffered => Phrase::new("leftover.skip.notOffered"),
            Self::Locked => Phrase::new("leftover.skip.locked"),
            Self::Gone => Phrase::new("leftover.skip.gone"),
            Self::Changed => Phrase::new("leftover.skip.changed"),
            Self::Link => Phrase::new("leftover.skip.link"),
            Self::Refused => Phrase::new("leftover.skip.refused"),
            Self::Failed(class) => {
                Phrase::new("leftover.skip.failed").phrase("reason", class_phrase(class))
            }
        }
    }
}

/// One find that was removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeftoverRemoved {
    /// The find's id.
    pub id: String,
    /// Folder or registry key.
    pub kind: LeftoverKind,
    /// The path or key.
    pub location: String,
    /// A folder's size in bytes.
    pub size_bytes: u64,
}

/// What removing the chosen finds did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LeftoverRemoval {
    /// What was removed.
    pub removed: Vec<LeftoverRemoved>,
    /// What was chosen but not removed, with why.
    pub skipped: Vec<(String, LeftoverSkip)>,
    /// The journal batches of the removed registry keys, for the Undo page.
    pub batches: Vec<BatchId>,
    /// Whether the activity record could not be written (the removal itself is unaffected).
    pub activity_write_failed: bool,
}

/// Why leftovers cannot be looked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum LeftoverError {
    /// Volt did not remove that app in this session, so it has no record of where the app lived.
    #[error("Volt has no record of removing that app")]
    NoRecord,
    /// This build has no way to look for leftovers.
    #[error("looking for leftovers is not available")]
    Unavailable,
}

/// What a find is made of, for removing it.
#[derive(Debug, Clone)]
enum Target {
    Folder(PathBuf),
    Key(RegistryKeyRef),
}

struct Found {
    item: Leftover,
    target: Target,
}

/// A candidate being collected: the strongest confidence wins and every piece of evidence counts.
struct Candidate<T> {
    target: T,
    confidence: Confidence,
    evidence: Vec<Phrase>,
}

/// What the scan compares a candidate against.
struct Context<'a> {
    neighbours: &'a [InstalledApp],
    own: &'a OwnInstall,
    protected: Vec<PathBuf>,
    windows: Option<PathBuf>,
    neighbour_folders: Vec<PathBuf>,
    neighbour_keys: Vec<RegistryKeyRef>,
}

/// Finds and removes what an uninstaller left, over the ports.
pub struct LeftoverCleaner {
    file_system: Arc<dyn FileSystem>,
    folders: Arc<dyn KnownFolders>,
    executor: Executor,
    clock: Arc<dyn Clock>,
    activity: Option<Arc<dyn ActivityLog>>,
}

impl std::fmt::Debug for LeftoverCleaner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LeftoverCleaner").finish_non_exhaustive()
    }
}

impl LeftoverCleaner {
    /// Looks at `file_system` and the registry of the executor's ports, resolves the user's folders
    /// through `folders` and removes registry keys through `executor` (so they are journaled).
    #[must_use]
    pub fn new(
        file_system: Arc<dyn FileSystem>,
        folders: Arc<dyn KnownFolders>,
        executor: Executor,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            file_system,
            folders,
            executor,
            clock,
            activity: None,
        }
    }

    /// Writes one activity record per removal to `log`.
    #[must_use]
    pub fn with_activity_log(mut self, log: Arc<dyn ActivityLog>) -> Self {
        self.activity = Some(log);
        self
    }

    fn registry(&self) -> &dyn Registry {
        self.executor.ports().registry.as_ref()
    }

    /// Looks for what the app left. Read-only. `neighbours` are the apps that are installed now.
    pub(super) fn scan(
        &self,
        footprint: &Footprint,
        neighbours: &[InstalledApp],
        own: &OwnInstall,
    ) -> LeftoverScan {
        let (found, incomplete) = self.find(footprint, neighbours, own);
        LeftoverScan {
            app_name: footprint.name.clone(),
            items: found.into_iter().map(|found| found.item).collect(),
            incomplete,
        }
    }

    /// Removes the finds `chosen` names (ids of a fresh scan): folders to the Recycle Bin after
    /// looking at them again, per-user registry keys through the journal.
    pub(super) fn remove(
        &self,
        footprint: &Footprint,
        neighbours: &[InstalledApp],
        own: &OwnInstall,
        chosen: &[String],
        source: Source,
    ) -> LeftoverRemoval {
        let (found, _) = self.find(footprint, neighbours, own);
        let mut removal = LeftoverRemoval::default();
        let mut wanted: Vec<&String> = Vec::new();
        for id in chosen {
            if !wanted.contains(&id) {
                wanted.push(id);
            }
        }
        let mut picked: Vec<&Found> = Vec::new();
        for id in wanted {
            match found.iter().find(|candidate| &candidate.item.id == id) {
                None => removal.skipped.push((id.clone(), LeftoverSkip::NotOffered)),
                Some(candidate) if candidate.item.locked.is_some() => {
                    removal.skipped.push((id.clone(), LeftoverSkip::Locked));
                }
                Some(candidate) => picked.push(candidate),
            }
        }
        // An outer folder goes first: a chosen folder inside it is then already gone.
        picked.sort_by_key(|candidate| candidate.item.location.len());
        let context = self.context(neighbours, own);
        for candidate in picked {
            let result = match &candidate.target {
                Target::Folder(path) => self.trash_folder(path, context.as_ref()),
                Target::Key(key) => self.delete_tree(key, &mut removal.batches, source),
            };
            match result {
                Ok(()) => removal.removed.push(LeftoverRemoved {
                    id: candidate.item.id.clone(),
                    kind: candidate.item.kind,
                    location: candidate.item.location.clone(),
                    size_bytes: candidate.item.size_bytes,
                }),
                Err(reason) => removal.skipped.push((candidate.item.id.clone(), reason)),
            }
        }
        removal.activity_write_failed = !self.record(&footprint.name, &removal, source);
        removal
    }

    // ---- finding ----------------------------------------------------------------------------

    fn find(
        &self,
        footprint: &Footprint,
        neighbours: &[InstalledApp],
        own: &OwnInstall,
    ) -> (Vec<Found>, bool) {
        let mut found = Vec::new();
        let mut incomplete = false;
        match self.context(neighbours, own) {
            Some(context) => {
                for candidate in self.folder_candidates(footprint, &context) {
                    let measured = self.measure(&candidate.target);
                    incomplete |= !measured.complete;
                    found.push(Found {
                        item: Leftover {
                            id: format!("folder:{}", candidate.target.display()),
                            kind: LeftoverKind::Folder,
                            location: candidate.target.display().to_string(),
                            confidence: candidate.confidence,
                            evidence: candidate.evidence,
                            size_bytes: measured.size,
                            items: measured.files,
                            locked: None,
                        },
                        target: Target::Folder(candidate.target),
                    });
                }
                for candidate in self.key_candidates(footprint, &context) {
                    let key = candidate.target;
                    let (locked, items) = if key.hive == RegistryHive::LocalMachine {
                        (Some(LeftoverLock::NeedsAdministrator), 0)
                    } else {
                        match self.key_tree(&key) {
                            Some(tree) => (None, u64::try_from(tree.len()).unwrap_or(u64::MAX)),
                            None => (Some(LeftoverLock::CannotRecord), 0),
                        }
                    };
                    found.push(Found {
                        item: Leftover {
                            id: format!("key:{key}"),
                            kind: LeftoverKind::RegistryKey,
                            location: key.to_string(),
                            confidence: candidate.confidence,
                            evidence: candidate.evidence,
                            size_bytes: 0,
                            items,
                            locked,
                        },
                        target: Target::Key(key),
                    });
                }
            }
            None => incomplete = true,
        }
        found.sort_by(|a, b| {
            b.item
                .confidence
                .cmp(&a.item.confidence)
                .then_with(|| a.item.kind.cmp(&b.item.kind))
                .then_with(|| a.item.location.cmp(&b.item.location))
        });
        (found, incomplete)
    }

    /// What every candidate is compared against. `None` when Windows would not say which folders
    /// are system folders: then no folder is offered at all.
    fn context<'a>(
        &self,
        neighbours: &'a [InstalledApp],
        own: &'a OwnInstall,
    ) -> Option<Context<'a>> {
        let mut protected = self.folders.system_folders().ok()?;
        protected.extend(self.folders.protected_folders().unwrap_or_default());
        for location in [
            Location::UserProfile,
            Location::LocalAppData,
            Location::RoamingAppData,
            Location::LocalLow,
            Location::ProgramData,
            Location::Windows,
        ] {
            protected.extend(self.folders.location(location).ok());
        }
        if let Ok(local) = self.folders.location(Location::LocalAppData) {
            protected.push(local.join("Programs"));
        }
        let mut context = Context {
            neighbours,
            own,
            protected,
            windows: self.folders.location(Location::Windows).ok(),
            neighbour_folders: Vec::new(),
            neighbour_keys: Vec::new(),
        };
        for footprint in neighbours.iter().filter_map(|app| app.footprint.as_ref()) {
            let folders: Vec<PathBuf> = footprint
                .folders
                .iter()
                .filter_map(|recorded| self.resolve(&recorded.raw))
                .collect();
            context.neighbour_folders.extend(folders);
            context
                .neighbour_keys
                .extend(footprint.keys.iter().cloned());
        }
        Some(context)
    }

    /// A recorded path as an absolute drive path, with `%VAR%` at its start expanded through the
    /// known folders. Anything else (relative, network, `..`, a name with forbidden characters)
    /// is `None`.
    fn resolve(&self, raw: &str) -> Option<PathBuf> {
        let text = raw.trim().trim_matches('"').trim().replace('/', "\\");
        let text = if let Some(rest) = text.strip_prefix('%') {
            let (variable, tail) = rest.split_once('%')?;
            if variable.is_empty()
                || !variable
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_')
            {
                return None;
            }
            let base = self.folders.env_folder(variable)?;
            format!("{}{tail}", base.display())
        } else {
            text
        };
        let text = text.trim_end_matches('\\');
        let bytes = text.as_bytes();
        if bytes.len() < 4
            || !bytes[0].is_ascii_alphabetic()
            || bytes[1] != b':'
            || bytes[2] != b'\\'
        {
            return None;
        }
        let tail = text.get(3..)?;
        let plain = tail.split('\\').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && !part.contains([':', '*', '?', '"', '<', '>', '|'])
                && !part.chars().any(char::is_control)
        });
        plain.then(|| PathBuf::from(text))
    }

    fn folder_candidates(
        &self,
        footprint: &Footprint,
        context: &Context<'_>,
    ) -> Vec<Candidate<PathBuf>> {
        let name = footprint.name.as_str();
        let mut candidates: BTreeMap<String, Candidate<PathBuf>> = BTreeMap::new();
        let mut add = |path: PathBuf, confidence: Confidence, why: Phrase| {
            let key = path.to_string_lossy().to_lowercase();
            let entry = candidates.entry(key).or_insert_with(|| Candidate {
                target: path,
                confidence,
                evidence: Vec::new(),
            });
            entry.confidence = entry.confidence.max(confidence);
            entry.evidence.push(why);
        };
        for recorded in &footprint.folders {
            let Some(path) = self.resolve(&recorded.raw) else {
                continue;
            };
            let (confidence, code) = match recorded.origin {
                FolderOrigin::InstallLocation => (Confidence::High, "leftover.why.installLocation"),
                FolderOrigin::Uninstaller => (Confidence::Medium, "leftover.why.uninstallerFolder"),
                FolderOrigin::Icon => (Confidence::Medium, "leftover.why.iconFolder"),
            };
            add(path, confidence, Phrase::new(code).text("name", name));
        }
        let mut roots = Vec::new();
        for location in [
            Location::LocalAppData,
            Location::RoamingAppData,
            Location::LocalLow,
        ] {
            roots.extend(self.folders.location(location).ok());
        }
        if let Ok(local) = self.folders.location(Location::LocalAppData) {
            roots.push(local.join("Programs"));
        }
        let product = usable_name(name);
        let publisher = footprint.publisher.as_deref().and_then(usable_name);
        for root in &roots {
            if let Some(product) = product {
                add(
                    root.join(product),
                    Confidence::Medium,
                    Phrase::new("leftover.why.sameName").text("name", name),
                );
            }
            if let Some(publisher) = publisher {
                add(
                    root.join(publisher),
                    Confidence::Low,
                    Phrase::new("leftover.why.publisherOnly")
                        .text("name", name)
                        .text("publisher", publisher),
                );
                if let Some(product) = product {
                    add(
                        root.join(publisher).join(product),
                        Confidence::Medium,
                        Phrase::new("leftover.why.publisherAndName")
                            .text("name", name)
                            .text("publisher", publisher),
                    );
                }
            }
        }
        let peer = peer_of(footprint, context);
        candidates
            .into_values()
            .filter(|candidate| !self.refused_folder(&candidate.target, context))
            .map(|mut candidate| {
                share(
                    &mut candidate.confidence,
                    &mut candidate.evidence,
                    candidate
                        .target
                        .file_name()
                        .and_then(|name| name.to_str())
                        .zip(footprint.publisher.as_deref())
                        .filter(|(folder, publisher)| folder.eq_ignore_ascii_case(publisher))
                        .and(peer),
                    footprint.publisher.as_deref(),
                );
                candidate
            })
            .collect()
    }

    /// Whether a folder is never offered: it is not a plain folder that exists, it is or sits
    /// behind a link, or it is somewhere removing would hurt (see the module documentation).
    fn refused_folder(&self, path: &Path, context: &Context<'_>) -> bool {
        if context
            .protected
            .iter()
            .any(|protected| starts_with_ci(protected, path))
            || context
                .windows
                .as_deref()
                .is_some_and(|windows| starts_with_ci(path, windows))
            || context
                .neighbour_folders
                .iter()
                .any(|other| starts_with_ci(path, other) || starts_with_ci(other, path))
            || context.own.overlaps(&path.to_string_lossy())
        {
            return true;
        }
        for ancestor in path.ancestors() {
            if ancestor.parent().is_none() {
                break;
            }
            match self.file_system.inspect(ancestor) {
                Ok(Some(info)) => {
                    if info.kind != EntryKind::Directory {
                        return true;
                    }
                }
                // The folder itself is not there; an ancestor that vanished is as good as that.
                Ok(None) | Err(_) => return true,
            }
        }
        false
    }

    /// Sizes a folder without following a link. Stops at [`MAX_ENTRIES`].
    fn measure(&self, path: &Path) -> Measured {
        let mut measured = Measured {
            size: 0,
            files: 0,
            complete: true,
        };
        let mut seen = 0_usize;
        let mut pending = vec![path.to_path_buf()];
        while let Some(dir) = pending.pop() {
            let Ok(entries) = self.file_system.read_dir(&dir) else {
                measured.complete = false;
                continue;
            };
            for entry in entries {
                seen += 1;
                if seen > MAX_ENTRIES {
                    measured.complete = false;
                    return measured;
                }
                match entry.info {
                    Ok(info) if info.kind == EntryKind::Directory => {
                        pending.push(dir.join(&entry.name));
                    }
                    Ok(info) => {
                        measured.size = measured.size.saturating_add(info.size_bytes);
                        measured.files += 1;
                    }
                    Err(_) => measured.complete = false,
                }
            }
        }
        measured
    }

    fn key_candidates(
        &self,
        footprint: &Footprint,
        context: &Context<'_>,
    ) -> Vec<Candidate<RegistryKeyRef>> {
        let name = footprint.name.as_str();
        let mut candidates: BTreeMap<String, Candidate<RegistryKeyRef>> = BTreeMap::new();
        let mut add = |key: RegistryKeyRef, confidence: Confidence, why: Phrase| {
            let id = format!("{}|{:?}", key.to_string().to_lowercase(), key.view);
            let entry = candidates.entry(id).or_insert_with(|| Candidate {
                target: key,
                confidence,
                evidence: Vec::new(),
            });
            entry.confidence = entry.confidence.max(confidence);
            entry.evidence.push(why);
        };
        for key in &footprint.keys {
            add(
                key.clone(),
                Confidence::High,
                Phrase::new("leftover.why.pointedKey").text("name", name),
            );
        }
        let product = usable_name(name);
        let publisher = footprint.publisher.as_deref().and_then(usable_name);
        let places = [
            (RegistryHive::CurrentUser, RegistryView::Native),
            (RegistryHive::LocalMachine, RegistryView::Bit64),
            (RegistryHive::LocalMachine, RegistryView::Bit32),
        ];
        for (hive, view) in places {
            let at = |path: String| RegistryKeyRef::new(hive, path, view);
            if let Some(product) = product {
                add(
                    at(format!(r"Software\{product}")),
                    Confidence::Medium,
                    Phrase::new("leftover.why.sameName").text("name", name),
                );
            }
            if let Some(publisher) = publisher {
                add(
                    at(format!(r"Software\{publisher}")),
                    Confidence::Low,
                    Phrase::new("leftover.why.publisherOnly")
                        .text("name", name)
                        .text("publisher", publisher),
                );
                if let Some(product) = product {
                    add(
                        at(format!(r"Software\{publisher}\{product}")),
                        Confidence::Medium,
                        Phrase::new("leftover.why.publisherAndName")
                            .text("name", name)
                            .text("publisher", publisher),
                    );
                }
            }
        }
        let peer = peer_of(footprint, context);
        candidates
            .into_values()
            .filter(|candidate| !refused_key(&candidate.target, candidate.confidence, context))
            .filter(|candidate| {
                let key = &candidate.target;
                self.registry()
                    .key_exists(key.hive, &key.subkey, key.view)
                    .unwrap_or(false)
            })
            .map(|mut candidate| {
                share(
                    &mut candidate.confidence,
                    &mut candidate.evidence,
                    candidate
                        .target
                        .subkey
                        .rsplit('\\')
                        .next()
                        .zip(footprint.publisher.as_deref())
                        .filter(|(last, publisher)| last.eq_ignore_ascii_case(publisher))
                        .and(peer),
                    footprint.publisher.as_deref(),
                );
                candidate
            })
            .collect()
    }

    // ---- removing ---------------------------------------------------------------------------

    fn trash_folder(&self, path: &Path, context: Option<&Context<'_>>) -> Result<(), LeftoverSkip> {
        let Some(context) = context else {
            return Err(LeftoverSkip::Refused);
        };
        match self.file_system.inspect(path) {
            Ok(Some(info)) if info.kind == EntryKind::Directory => {}
            Ok(Some(info)) if info.kind == EntryKind::ReparsePoint => {
                return Err(LeftoverSkip::Link);
            }
            Ok(Some(_)) => return Err(LeftoverSkip::Changed),
            Ok(None) => return Err(LeftoverSkip::Gone),
            Err(error) => return Err(LeftoverSkip::Failed(error_class(&error))),
        }
        if self.refused_folder(path, context) {
            return Err(LeftoverSkip::Refused);
        }
        let parent = path.parent().ok_or(LeftoverSkip::Refused)?;
        self.file_system
            .move_to_trash(parent, path)
            .map_err(|error| match error {
                PortError::NotFound(_) => LeftoverSkip::Gone,
                PortError::InvalidInput(_) => LeftoverSkip::Link,
                other => LeftoverSkip::Failed(error_class(&other)),
            })
    }

    /// The key and every key below it, deepest first, or `None` when the tree is too big or holds
    /// more than Volt can save, or could not be read.
    fn key_tree(&self, key: &RegistryKeyRef) -> Option<Vec<RegistryKeyRef>> {
        let mut tree = Vec::new();
        self.collect_keys(key, 0, &mut tree)?;
        Some(tree)
    }

    fn collect_keys(
        &self,
        key: &RegistryKeyRef,
        depth: usize,
        tree: &mut Vec<RegistryKeyRef>,
    ) -> Option<()> {
        if depth > MAX_DEPTH || tree.len() >= MAX_KEYS {
            return None;
        }
        if !values_recordable(self.registry(), key).ok()? {
            return None;
        }
        for name in self.registry().list_subkeys(key).ok()? {
            let child = RegistryKeyRef::new(key.hive, format!(r"{}\{name}", key.subkey), key.view);
            self.collect_keys(&child, depth + 1, tree)?;
        }
        tree.push(key.clone());
        (tree.len() <= MAX_KEYS).then_some(())
    }

    fn delete_tree(
        &self,
        key: &RegistryKeyRef,
        batches: &mut Vec<BatchId>,
        source: Source,
    ) -> Result<(), LeftoverSkip> {
        if key.hive != RegistryHive::CurrentUser {
            return Err(LeftoverSkip::Locked);
        }
        match self.registry().key_exists(key.hive, &key.subkey, key.view) {
            Ok(true) => {}
            Ok(false) => return Err(LeftoverSkip::Gone),
            Err(error) => return Err(LeftoverSkip::Failed(error_class(&error))),
        }
        let tree = self.key_tree(key).ok_or(LeftoverSkip::Locked)?;
        let requests = tree
            .into_iter()
            .map(|key| {
                OperationRequest::new(
                    Operation::DeleteRegistryKey { key },
                    Risk::Caution,
                    Evidence::Measured,
                )
            })
            .collect();
        let plan = Plan::build(requests, self.executor.ports())
            .map_err(|PlanError::Detect { error, .. }| LeftoverSkip::Failed(error_class(&error)))?
            .with_label(LEFTOVERS_LABEL);
        let report = self
            .executor
            .apply_or_rollback(&plan, source)
            .map_err(|error| LeftoverSkip::Failed(exec_class(&error)))?;
        if report.is_success() {
            batches.push(report.batch);
            return Ok(());
        }
        let class = report
            .outcomes
            .iter()
            .find_map(|outcome| match &outcome.status {
                ItemStatus::Failed { error, .. } => Some(exec_class(error)),
                _ => None,
            })
            .unwrap_or("error");
        Err(LeftoverSkip::Failed(class))
    }

    /// Writes the activity record. `true` when it was written or there is nothing to write.
    fn record(&self, name: &str, removal: &LeftoverRemoval, source: Source) -> bool {
        let Some(log) = &self.activity else {
            return true;
        };
        if removal.removed.is_empty() && removal.skipped.is_empty() {
            return true;
        }
        let phrase = if removal.removed.is_empty() {
            Phrase::new("summary.leftovers.none").text("name", name)
        } else {
            let mut parts: Vec<Phrase> = removal
                .removed
                .iter()
                .take(MAX_LISTED)
                .map(|removed| match removed.kind {
                    LeftoverKind::Folder => Phrase::new("summary.leftovers.folder")
                        .text("path", removed.location.clone())
                        .size("size", removed.size_bytes),
                    LeftoverKind::RegistryKey => {
                        Phrase::new("summary.leftovers.key").text("path", removed.location.clone())
                    }
                })
                .collect();
            if removal.removed.len() > MAX_LISTED {
                parts.push(
                    Phrase::new("summary.leftovers.more")
                        .count("count", removal.removed.len() - MAX_LISTED),
                );
            }
            Phrase::new("summary.leftovers")
                .text("name", name)
                .list("parts", ", ", parts)
                .maybe(
                    "left",
                    (!removal.skipped.is_empty()).then(|| {
                        Phrase::new("summary.leftovers.left").count("count", removal.skipped.len())
                    }),
                )
        };
        let outcome = if removal.removed.is_empty() {
            ActivityOutcome::Failed
        } else {
            ActivityOutcome::Applied
        };
        let activity = NewActivity::said(
            self.clock.now(),
            source,
            outcome,
            phrase,
            Some(LEFTOVERS_LABEL.to_owned()),
        );
        match log.record(activity) {
            Ok(_) => true,
            Err(error) => {
                log::warn!(
                    "could not write the activity record for removing leftovers ({})",
                    error_class(&error)
                );
                false
            }
        }
    }
}

struct Measured {
    size: u64,
    files: u64,
    complete: bool,
}

/// A name that is safe to look for as one folder or key name: long enough, one plain component.
fn usable_name(name: &str) -> Option<&str> {
    let name = name.trim();
    let plain = name.chars().count() >= MIN_NAME
        && name.len() <= 100
        && name != "."
        && name != ".."
        && !name.ends_with(['.', ' '])
        && !name.contains(['\\', '/', ':', '*', '?', '"', '<', '>', '|'])
        && !name.chars().any(char::is_control);
    plain.then_some(name)
}

/// An installed app from the same publisher as the removed one, if there is one.
fn peer_of<'a>(footprint: &Footprint, context: &'a Context<'_>) -> Option<&'a str> {
    let publisher = footprint.publisher.as_deref()?.trim();
    if publisher.is_empty() {
        return None;
    }
    context
        .neighbours
        .iter()
        .find(|app| {
            app.publisher
                .as_deref()
                .is_some_and(|other| other.trim().eq_ignore_ascii_case(publisher))
        })
        .map(|app| app.name.as_str())
}

/// A find named like the publisher while another app of that publisher is installed is shared with
/// it: never high.
fn share(
    confidence: &mut Confidence,
    evidence: &mut Vec<Phrase>,
    peer: Option<&str>,
    publisher: Option<&str>,
) {
    if let (Some(peer), Some(publisher)) = (peer, publisher) {
        *confidence = (*confidence).min(Confidence::Low);
        evidence.push(
            Phrase::new("leftover.why.shared")
                .text("app", peer)
                .text("publisher", publisher),
        );
    }
}

/// Whether a registry key is never offered.
fn refused_key(key: &RegistryKeyRef, confidence: Confidence, context: &Context<'_>) -> bool {
    if !matches!(
        key.hive,
        RegistryHive::CurrentUser | RegistryHive::LocalMachine
    ) {
        return true;
    }
    let parts: Vec<&str> = key.subkey.split('\\').collect();
    if parts.iter().any(|part| part.is_empty()) || !parts[0].eq_ignore_ascii_case("software") {
        return true;
    }
    // Below `Software`: the vendor, then the product. A key the entry named must be a product
    // key, not a whole vendor's.
    let below: &[&str] = &parts[1..];
    let Some(vendor) = below.first() else {
        return true;
    };
    if PROTECTED_VENDORS
        .iter()
        .any(|protected| vendor.eq_ignore_ascii_case(protected))
        || (confidence == Confidence::High && below.len() < 2)
    {
        return true;
    }
    if context
        .own
        .is_volt(parts[parts.len() - 1], std::iter::empty())
    {
        return true;
    }
    context.neighbour_keys.iter().any(|other| {
        other.hive == key.hive && {
            let (a, b) = (key.subkey.to_lowercase(), other.subkey.to_lowercase());
            a == b
                || a.strip_prefix(&b)
                    .is_some_and(|rest| rest.starts_with('\\'))
                || b.strip_prefix(&a)
                    .is_some_and(|rest| rest.starts_with('\\'))
        }
    })
}

/// The class of a failed change, without any message a port wrote.
fn exec_class(error: &ExecError) -> &'static str {
    match error {
        ExecError::Detect(error)
        | ExecError::Journal(error)
        | ExecError::MarkApplied(error)
        | ExecError::Apply { error, .. } => error_class(error),
    }
}
