//! In-memory fakes of every port, for tests that must not touch the real system.
//!
//! Available to this crate's own tests and to other crates through the `fakes`
//! cargo feature. [`FakeSystem`] wires them together with a shared
//! [`EventLog`], so a test can prove the order of effects (for example that the
//! journal entry was written before the registry was mutated).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::activity::{ActivityId, ActivityLog, ActivityRecord, NewActivity, Retention};
use crate::command::{Command, CommandOutput, Commands};
use crate::error::PortError;
use crate::exclusions::ExclusionList;
use crate::executor::Executor;
use crate::journal::{BatchId, EntryId, EntryState, Journal, JournalEntry, NewEntry};
use crate::ports::{
    Availability, DirEntry, EntryInfo, EntryKind, FileIdentity, FileStat, FileSystem, Memory,
    MemoryArea, MemoryLists, MemoryStatus, Ports, ProcessInfo, Processes, PurgeOutcome, Services,
};
use crate::registry::{
    NamedValue, Registry, RegistryHive, RegistryKeyRef, RegistryValue, RegistryValueRef,
    RegistryView,
};
use crate::service::ServiceStartType;
use crate::settings::Settings;
use crate::time::{Clock, Timestamp};
use crate::tuning::{PowerThrottling, PriorityClass, ProcessChange, ProcessSettings};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A side effect observed by a fake, in the order it happened.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Event {
    /// The journal stored a new pending entry.
    JournalPending(EntryId),
    /// The journal changed an entry's state.
    JournalState(EntryId, EntryState),
    /// A registry value was written.
    RegistryWrite(RegistryValueRef),
    /// A registry value was deleted.
    RegistryDelete(RegistryValueRef),
    /// A registry key was created.
    RegistryKeyCreate(RegistryKeyRef),
    /// A registry key was deleted.
    RegistryKeyDelete(RegistryKeyRef),
    /// A service start type was changed.
    ServiceSetStartType(String, ServiceStartType),
    /// A path was moved to the trash.
    FileTrashed(PathBuf),
    /// A process was terminated.
    ProcessTerminated(u32),
    /// The settings of a process were changed.
    ProcessTuned(u32),
    /// A memory area was purged.
    MemoryPurged(MemoryArea),
    /// A command that changes something was run (detect commands are reads
    /// and are not logged).
    CommandRun(Command),
}

/// Shared, ordered record of the side effects of all fakes built with it.
#[derive(Debug, Clone, Default)]
pub struct EventLog(Arc<Mutex<Vec<Event>>>);

impl EventLog {
    /// Appends an event.
    pub fn push(&self, event: Event) {
        lock(&self.0).push(event);
    }

    /// A copy of all events so far.
    #[must_use]
    pub fn events(&self) -> Vec<Event> {
        lock(&self.0).clone()
    }
}

/// Case-insensitive registry key, like the real registry.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Key(RegistryHive, RegistryView, String, String);

impl Key {
    fn of(target: &RegistryValueRef) -> Self {
        Self(
            target.hive,
            target.view,
            target.subkey.to_lowercase(),
            target.name.to_lowercase(),
        )
    }
}

/// A key as the fake tracks it: hive, view and lower-cased subkey path.
type KeyPath = (RegistryHive, RegistryView, String);

/// In-memory [`Registry`].
#[derive(Debug, Default)]
pub struct FakeRegistry {
    values: Mutex<HashMap<Key, RegistryValue>>,
    /// The spelling of each value name as it was written, for listings.
    names: Mutex<HashMap<Key, String>>,
    failures: Mutex<HashMap<Key, PortError>>,
    /// Every key that exists, with its parents, also when it holds no value (see
    /// [`FakeRegistry::add_key`]). Writing a value creates its key and keeps it after the value is
    /// deleted, like the real registry.
    keys: Mutex<HashSet<KeyPath>>,
    key_failures: Mutex<HashMap<KeyPath, PortError>>,
    log: EventLog,
}

impl FakeRegistry {
    /// An empty registry with its own event log.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// An empty registry reporting to a shared event log.
    #[must_use]
    pub fn with_log(log: EventLog) -> Self {
        Self {
            log,
            ..Self::default()
        }
    }

    /// Sets a value directly, without logging. For arranging a test.
    pub fn set(&self, target: &RegistryValueRef, value: RegistryValue) {
        self.store(target, value);
    }

    fn store(&self, target: &RegistryValueRef, value: RegistryValue) {
        let key = Key::of(target);
        self.add_path(target.hive, target.view, &key.2);
        lock(&self.names).insert(key.clone(), target.name.clone());
        lock(&self.values).insert(key, value);
    }

    /// Makes a key and its parents exist. `path` is lower-case.
    fn add_path(&self, hive: RegistryHive, view: RegistryView, path: &str) {
        let mut keys = lock(&self.keys);
        let mut prefix = String::new();
        for part in path.split('\\').filter(|part| !part.is_empty()) {
            if !prefix.is_empty() {
                prefix.push('\\');
            }
            prefix.push_str(part);
            keys.insert((hive, view, prefix.clone()));
        }
    }

    /// Reads a value directly. For asserting in a test.
    #[must_use]
    pub fn get(&self, target: &RegistryValueRef) -> Option<RegistryValue> {
        lock(&self.values).get(&Key::of(target)).cloned()
    }

    /// Makes every write and delete of `target` fail with `error`.
    pub fn fail_on(&self, target: &RegistryValueRef, error: PortError) {
        lock(&self.failures).insert(Key::of(target), error);
    }

    /// Removes every failure set with [`FakeRegistry::fail_on`].
    pub fn clear_failures(&self) {
        lock(&self.failures).clear();
    }

    /// Makes `subkey` exist without any value in it, like a marker key. Without logging. For
    /// arranging a test.
    pub fn add_key(&self, hive: RegistryHive, subkey: &str, view: RegistryView) {
        self.add_path(hive, view, &subkey.to_lowercase());
    }

    /// Makes every [`Registry::key_exists`] of `subkey` fail with `error`.
    pub fn fail_key_exists(
        &self,
        hive: RegistryHive,
        subkey: &str,
        view: RegistryView,
        error: PortError,
    ) {
        lock(&self.key_failures).insert((hive, view, subkey.to_lowercase()), error);
    }

    fn check_failure(&self, key: &Key) -> Result<(), PortError> {
        match lock(&self.failures).get(key) {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        }
    }
}

impl Registry for FakeRegistry {
    fn list_values(&self, key: &RegistryKeyRef) -> Result<Vec<NamedValue>, PortError> {
        let wanted = key.subkey.to_lowercase();
        let values = lock(&self.values);
        let names = lock(&self.names);
        let mut listing: Vec<NamedValue> = values
            .iter()
            .filter(|(k, _)| k.0 == key.hive && k.1 == key.view && k.2 == wanted)
            .map(|(k, value)| NamedValue {
                name: names.get(k).cloned().unwrap_or_else(|| k.3.clone()),
                value: Ok(value.clone()),
            })
            .collect();
        listing.sort_by_key(|entry| entry.name.to_lowercase());
        Ok(listing)
    }

    fn read_value(&self, target: &RegistryValueRef) -> Result<Option<RegistryValue>, PortError> {
        Ok(self.get(target))
    }

    fn write_value(
        &self,
        target: &RegistryValueRef,
        data: &RegistryValue,
    ) -> Result<(), PortError> {
        let key = Key::of(target);
        self.check_failure(&key)?;
        self.store(target, data.clone());
        self.log.push(Event::RegistryWrite(target.clone()));
        Ok(())
    }

    fn delete_value(&self, target: &RegistryValueRef) -> Result<(), PortError> {
        let key = Key::of(target);
        self.check_failure(&key)?;
        if lock(&self.values).remove(&key).is_none() {
            return Err(PortError::NotFound(target.to_string()));
        }
        lock(&self.names).remove(&key);
        self.log.push(Event::RegistryDelete(target.clone()));
        Ok(())
    }

    fn key_exists(
        &self,
        hive: RegistryHive,
        subkey: &str,
        view: RegistryView,
    ) -> Result<bool, PortError> {
        let path = (hive, view, subkey.to_lowercase());
        if let Some(error) = lock(&self.key_failures).get(&path) {
            return Err(error.clone());
        }
        Ok(lock(&self.keys).contains(&path))
    }

    fn list_subkeys(&self, key: &RegistryKeyRef) -> Result<Vec<String>, PortError> {
        let prefix = format!("{}\\", key.subkey.to_lowercase());
        let mut names: Vec<String> = lock(&self.keys)
            .iter()
            .filter(|(hive, view, _)| *hive == key.hive && *view == key.view)
            .filter_map(|(_, _, path)| path.strip_prefix(&prefix))
            .filter(|rest| !rest.contains('\\'))
            .map(str::to_owned)
            .collect();
        names.sort();
        Ok(names)
    }

    fn create_key(&self, key: &RegistryKeyRef) -> Result<(), PortError> {
        if key.subkey.is_empty() {
            return Err(PortError::InvalidInput(
                "a registry key to create or delete must be below a hive".to_owned(),
            ));
        }
        let path = (key.hive, key.view, key.subkey.to_lowercase());
        let existed = lock(&self.keys).contains(&path);
        self.add_path(key.hive, key.view, &path.2);
        if !existed {
            self.log.push(Event::RegistryKeyCreate(key.clone()));
        }
        Ok(())
    }

    fn delete_key(&self, key: &RegistryKeyRef) -> Result<(), PortError> {
        if key.subkey.is_empty() {
            return Err(PortError::InvalidInput(
                "a registry key to create or delete must be below a hive".to_owned(),
            ));
        }
        let path = (key.hive, key.view, key.subkey.to_lowercase());
        if !lock(&self.keys).contains(&path) {
            return Err(PortError::NotFound(key.to_string()));
        }
        if !self.list_subkeys(key)?.is_empty() {
            return Err(PortError::Other(format!("{key} has subkeys")));
        }
        let mut values = lock(&self.values);
        let doomed: Vec<Key> = values
            .keys()
            .filter(|k| k.0 == key.hive && k.1 == key.view && k.2 == path.2)
            .cloned()
            .collect();
        let mut names = lock(&self.names);
        for value in doomed {
            values.remove(&value);
            names.remove(&value);
        }
        lock(&self.keys).remove(&path);
        self.log.push(Event::RegistryKeyDelete(key.clone()));
        Ok(())
    }
}

/// In-memory [`Services`].
#[derive(Debug, Default)]
pub struct FakeServices {
    services: Mutex<HashMap<String, ServiceStartType>>,
    failures: Mutex<HashMap<String, PortError>>,
    log: EventLog,
}

impl FakeServices {
    /// No services, own event log.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// No services, reporting to a shared event log.
    #[must_use]
    pub fn with_log(log: EventLog) -> Self {
        Self {
            log,
            ..Self::default()
        }
    }

    /// Adds or replaces a service directly, without logging.
    pub fn add(&self, service: &str, start_type: ServiceStartType) {
        lock(&self.services).insert(service.to_owned(), start_type);
    }

    /// Makes every `set_start_type` for `service` fail with `error`.
    pub fn fail_on(&self, service: &str, error: PortError) {
        lock(&self.failures).insert(service.to_owned(), error);
    }
}

impl Services for FakeServices {
    fn start_type(&self, service: &str) -> Result<ServiceStartType, PortError> {
        lock(&self.services)
            .get(service)
            .copied()
            .ok_or_else(|| PortError::NotFound(format!("service {service}")))
    }

    fn set_start_type(&self, service: &str, start_type: ServiceStartType) -> Result<(), PortError> {
        if let Some(error) = lock(&self.failures).get(service) {
            return Err(error.clone());
        }
        match lock(&self.services).get_mut(service) {
            Some(slot) => *slot = start_type,
            None => return Err(PortError::NotFound(format!("service {service}"))),
        }
        self.log
            .push(Event::ServiceSetStartType(service.to_owned(), start_type));
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
struct FakeEntry {
    kind: EntryKind,
    size_bytes: u64,
    modified: Option<Timestamp>,
    system: bool,
}

impl FakeEntry {
    fn info(self) -> EntryInfo {
        EntryInfo {
            kind: self.kind,
            size_bytes: self.size_bytes,
            modified: self.modified,
            system: self.system,
        }
    }
}

#[derive(Debug, Default)]
struct FileSystemState {
    entries: HashMap<PathBuf, FakeEntry>,
    trashed: Vec<PathBuf>,
    trash_attempts: Vec<PathBuf>,
    read_dirs: Vec<PathBuf>,
    trash_failures: HashMap<PathBuf, PortError>,
    read_dir_failures: HashMap<PathBuf, PortError>,
    contents: HashMap<PathBuf, Vec<u8>>,
    identities: HashMap<PathBuf, FileIdentity>,
    placeholders: HashSet<PathBuf>,
    bytes_read: HashMap<PathBuf, u64>,
    placeholder_reads: Vec<PathBuf>,
}

/// In-memory [`FileSystem`].
///
/// Entries are added with [`FakeFileSystem::add_file`], [`add_dir`] and
/// [`add_link`]; missing parent directories are created as plain directories.
/// A link is a reparse point: it may have entries below it (what a real
/// junction would show), which lets a test prove they are never reached.
///
/// [`add_dir`]: FakeFileSystem::add_dir
/// [`add_link`]: FakeFileSystem::add_link
#[derive(Debug, Default)]
pub struct FakeFileSystem {
    state: Mutex<FileSystemState>,
    log: EventLog,
}

impl FakeFileSystem {
    /// Empty file system, own event log.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Empty file system, reporting to a shared event log.
    #[must_use]
    pub fn with_log(log: EventLog) -> Self {
        Self {
            log,
            ..Self::default()
        }
    }

    fn insert(&self, path: PathBuf, entry: FakeEntry) {
        let mut state = lock(&self.state);
        for ancestor in path.ancestors().skip(1) {
            if ancestor.as_os_str().is_empty() {
                continue;
            }
            state
                .entries
                .entry(ancestor.to_path_buf())
                .or_insert(FakeEntry {
                    kind: EntryKind::Directory,
                    size_bytes: 0,
                    modified: None,
                    system: false,
                });
        }
        state.entries.insert(path, entry);
    }

    /// Adds (or replaces) a file of `size_bytes` with no modification time.
    pub fn add_file(&self, path: impl Into<PathBuf>, size_bytes: u64) {
        self.insert(
            path.into(),
            FakeEntry {
                kind: EntryKind::File,
                size_bytes,
                modified: None,
                system: false,
            },
        );
    }

    /// Adds (or replaces) a file of `size_bytes` last modified at `modified`.
    pub fn add_file_at(&self, path: impl Into<PathBuf>, size_bytes: u64, modified: Timestamp) {
        self.insert(
            path.into(),
            FakeEntry {
                kind: EntryKind::File,
                size_bytes,
                modified: Some(modified),
                system: false,
            },
        );
    }

    /// Adds a file that has the system attribute.
    pub fn add_system_file(&self, path: impl Into<PathBuf>, size_bytes: u64) {
        self.insert(
            path.into(),
            FakeEntry {
                kind: EntryKind::File,
                size_bytes,
                modified: None,
                system: true,
            },
        );
    }

    /// Adds a directory last modified at `modified` (when given).
    pub fn add_dir(&self, path: impl Into<PathBuf>, modified: Option<Timestamp>) {
        self.insert(
            path.into(),
            FakeEntry {
                kind: EntryKind::Directory,
                size_bytes: 0,
                modified,
                system: false,
            },
        );
    }

    /// Adds a reparse point (a junction or symbolic link).
    pub fn add_link(&self, path: impl Into<PathBuf>) {
        self.insert(
            path.into(),
            FakeEntry {
                kind: EntryKind::ReparsePoint,
                size_bytes: 0,
                modified: None,
                system: false,
            },
        );
    }

    /// Paths moved to the trash so far, in order.
    #[must_use]
    pub fn trashed(&self) -> Vec<PathBuf> {
        lock(&self.state).trashed.clone()
    }

    /// Every `move_to_trash` request that passed the containment checks, in
    /// order, whether it then succeeded or failed.
    #[must_use]
    pub fn trash_attempts(&self) -> Vec<PathBuf> {
        lock(&self.state).trash_attempts.clone()
    }

    /// Directories listed with `read_dir` so far, in order.
    #[must_use]
    pub fn read_dir_calls(&self) -> Vec<PathBuf> {
        lock(&self.state).read_dirs.clone()
    }

    /// Makes `move_to_trash` of `path` fail with `error`, as a locked file
    /// would.
    pub fn fail_trash_of(&self, path: impl Into<PathBuf>, error: PortError) {
        lock(&self.state).trash_failures.insert(path.into(), error);
    }

    /// Makes `read_dir` of `dir` fail with `error`.
    pub fn fail_read_dir_of(&self, dir: impl Into<PathBuf>, error: PortError) {
        lock(&self.state)
            .read_dir_failures
            .insert(dir.into(), error);
    }

    /// Whether anything exists at `path` (without following links).
    #[must_use]
    pub fn exists(&self, path: &Path) -> bool {
        lock(&self.state).entries.contains_key(path)
    }

    /// Adds (or replaces) a file whose data is `content`; its size is the length of `content`.
    pub fn add_file_with_content(&self, path: impl Into<PathBuf>, content: &[u8]) {
        let path = path.into();
        self.add_file(path.clone(), content.len() as u64);
        lock(&self.state).contents.insert(path, content.to_vec());
    }

    /// Replaces the data a file reads back, leaving its listed size and time as they are, so a
    /// test can change a file behind an unchanged listing.
    pub fn set_content(&self, path: impl Into<PathBuf>, content: &[u8]) {
        lock(&self.state)
            .contents
            .insert(path.into(), content.to_vec());
    }

    /// Adds a file that is a cloud placeholder: its data is not on disk, so it must never be
    /// read.
    pub fn add_placeholder(&self, path: impl Into<PathBuf>, size_bytes: u64) {
        let path = path.into();
        self.add_file(path.clone(), size_bytes);
        lock(&self.state).placeholders.insert(path);
    }

    /// Makes `path` another name of the file `(volume, file_id)`, like a hard link.
    pub fn set_identity(&self, path: impl Into<PathBuf>, volume: u64, file_id: u64, links: u32) {
        lock(&self.state).identities.insert(
            path.into(),
            FileIdentity {
                volume,
                file_id,
                links,
                on_disk: true,
            },
        );
    }

    /// How many bytes `read_at` returned for `path` so far.
    #[must_use]
    pub fn bytes_read_of(&self, path: &Path) -> u64 {
        lock(&self.state)
            .bytes_read
            .get(path)
            .copied()
            .unwrap_or_default()
    }

    /// How many times a placeholder was asked to be read (always refused). Must stay 0.
    #[must_use]
    pub fn placeholder_reads(&self) -> usize {
        lock(&self.state).placeholder_reads.len()
    }
}

impl FileSystem for FakeFileSystem {
    fn stat(&self, path: &Path) -> Result<Option<FileStat>, PortError> {
        Ok(lock(&self.state).entries.get(path).map(|entry| FileStat {
            size_bytes: entry.size_bytes,
            is_dir: entry.kind == EntryKind::Directory,
        }))
    }

    fn inspect(&self, path: &Path) -> Result<Option<EntryInfo>, PortError> {
        Ok(lock(&self.state)
            .entries
            .get(path)
            .map(|entry| entry.info()))
    }

    fn read_dir(&self, dir: &Path) -> Result<Vec<DirEntry>, PortError> {
        let mut state = lock(&self.state);
        state.read_dirs.push(dir.to_path_buf());
        if let Some(error) = state.read_dir_failures.get(dir) {
            return Err(error.clone());
        }
        match state.entries.get(dir).map(|entry| entry.kind) {
            None => return Err(PortError::NotFound(dir.display().to_string())),
            Some(EntryKind::Directory) => {}
            Some(_) => {
                return Err(PortError::InvalidInput(format!(
                    "{} is not a plain directory",
                    dir.display()
                )));
            }
        }
        let mut listing: Vec<DirEntry> = state
            .entries
            .iter()
            .filter(|(path, _)| path.parent() == Some(dir))
            .filter_map(|(path, entry)| {
                Some(DirEntry {
                    name: path.file_name()?.to_os_string(),
                    info: Ok(entry.info()),
                })
            })
            .collect();
        listing.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(listing)
    }

    fn move_to_trash(&self, root: &Path, path: &Path) -> Result<(), PortError> {
        let refuse = |why: &str| {
            Err(PortError::InvalidInput(format!(
                "{} {why} {}",
                path.display(),
                root.display()
            )))
        };
        let Ok(relative) = path.strip_prefix(root) else {
            return refuse("is outside");
        };
        if relative.as_os_str().is_empty() {
            return refuse("is the root itself:");
        }
        if relative
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            return refuse("is not a plain path below");
        }
        let mut state = lock(&self.state);
        // Like the real adapter: nothing is reached through a link.
        let mut current = root.to_path_buf();
        for component in relative.components() {
            current.push(component);
            if state
                .entries
                .get(&current)
                .is_some_and(|entry| entry.kind == EntryKind::ReparsePoint)
            {
                return refuse("is behind a junction or symbolic link, outside");
            }
        }
        state.trash_attempts.push(path.to_path_buf());
        if let Some(error) = state.trash_failures.get(path) {
            return Err(error.clone());
        }
        if !state.entries.contains_key(path) {
            return Err(PortError::NotFound(path.display().to_string()));
        }
        state
            .entries
            .retain(|candidate, _| !candidate.starts_with(path));
        state.trashed.push(path.to_path_buf());
        drop(state);
        self.log.push(Event::FileTrashed(path.to_path_buf()));
        Ok(())
    }

    fn identity(&self, path: &Path) -> Result<FileIdentity, PortError> {
        let state = lock(&self.state);
        match state.entries.get(path) {
            None => Err(PortError::NotFound(path.display().to_string())),
            Some(entry) if entry.kind != EntryKind::File => Err(PortError::InvalidInput(format!(
                "{} is not a plain file",
                path.display()
            ))),
            Some(_) => {
                let on_disk = !state.placeholders.contains(path);
                Ok(state.identities.get(path).map_or_else(
                    || {
                        use std::hash::{Hash as _, Hasher as _};
                        let mut hasher = std::collections::hash_map::DefaultHasher::new();
                        path.hash(&mut hasher);
                        FileIdentity {
                            volume: 1,
                            file_id: hasher.finish(),
                            links: 1,
                            on_disk,
                        }
                    },
                    |identity| FileIdentity {
                        on_disk,
                        ..*identity
                    },
                ))
            }
        }
    }

    fn read_at(&self, path: &Path, offset: u64, buf: &mut [u8]) -> Result<usize, PortError> {
        let mut state = lock(&self.state);
        if state.placeholders.contains(path) {
            state.placeholder_reads.push(path.to_path_buf());
            return Err(PortError::Unsupported("the file is not on disk".to_owned()));
        }
        match state.entries.get(path).map(|entry| entry.kind) {
            None => return Err(PortError::NotFound(path.display().to_string())),
            Some(EntryKind::File) => {}
            Some(_) => {
                return Err(PortError::InvalidInput(format!(
                    "{} is not a plain file",
                    path.display()
                )));
            }
        }
        let Some(content) = state.contents.get(path) else {
            return Err(PortError::NotFound(format!("data of {}", path.display())));
        };
        let start = usize::try_from(offset)
            .unwrap_or(usize::MAX)
            .min(content.len());
        let count = buf.len().min(content.len() - start);
        buf[..count].copy_from_slice(&content[start..start + count]);
        *state.bytes_read.entry(path.to_path_buf()).or_default() += count as u64;
        Ok(count)
    }
}

/// In-memory [`Processes`].
#[derive(Debug, Default)]
pub struct FakeProcesses {
    processes: Mutex<Vec<ProcessInfo>>,
    terminate_failures: Mutex<HashMap<u32, PortError>>,
    /// Creation times `list` reports instead of the real ones (a pid reused after the listing).
    listed_creation: Mutex<HashMap<u32, u64>>,
    /// Settings of processes that differ from [`FakeProcesses::DEFAULT_SETTINGS`].
    tuned: Mutex<HashMap<u32, ProcessSettings>>,
    /// Errors `settings` returns for a pid.
    settings_failures: Mutex<HashMap<u32, PortError>>,
    /// Errors `apply_settings` returns for a pid (nothing is changed then).
    apply_failures: Mutex<HashMap<u32, PortError>>,
    log: EventLog,
}

impl FakeProcesses {
    /// What a process holds until a test or a change sets it: normal priority, eight cores,
    /// power throttling left to Windows.
    pub const DEFAULT_SETTINGS: ProcessSettings = ProcessSettings {
        priority: PriorityClass::Normal,
        affinity_mask: 0xFF,
        system_affinity_mask: 0xFF,
        throttling: PowerThrottling::Default,
    };

    /// No processes, own event log.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// No processes, reporting to a shared event log.
    #[must_use]
    pub fn with_log(log: EventLog) -> Self {
        Self {
            log,
            ..Self::default()
        }
    }

    /// Adds a running process with no parent and no CPU time.
    pub fn add(&self, pid: u32, name: &str, working_set_bytes: u64) {
        self.add_process(pid, 0, name, working_set_bytes, 0);
    }

    /// Adds a running process with its parent and the CPU time it has used so far.
    pub fn add_process(
        &self,
        pid: u32,
        parent_pid: u32,
        name: &str,
        working_set_bytes: u64,
        cpu_time_100ns: u64,
    ) {
        lock(&self.processes).push(ProcessInfo {
            pid,
            parent_pid,
            name: name.to_owned(),
            working_set_bytes,
            cpu_time_100ns,
            created_at_100ns: 0,
            path: None,
            session_id: 1,
        });
    }

    /// Sets the executable path of a running process.
    pub fn set_path(&self, pid: u32, path: &str) {
        self.update(pid, |p| p.path = Some(path.to_owned()));
    }

    /// Sets the Windows session of a running process.
    pub fn set_session(&self, pid: u32, session_id: u32) {
        self.update(pid, |p| p.session_id = session_id);
    }

    /// Sets when a running process started.
    pub fn set_created(&self, pid: u32, created_at_100ns: u64) {
        self.update(pid, |p| p.created_at_100ns = created_at_100ns);
    }

    /// Makes `list` report `shown` as the creation time of `pid` while the real process keeps its
    /// own: the listing is stale, as if the id had been reused after it was read.
    pub fn list_stale_creation(&self, pid: u32, shown: u64) {
        lock(&self.listed_creation).insert(pid, shown);
    }

    fn update(&self, pid: u32, change: impl Fn(&mut ProcessInfo)) {
        for process in lock(&self.processes).iter_mut() {
            if process.pid == pid {
                change(process);
            }
        }
    }

    /// Sets the CPU time a running process has used so far.
    pub fn set_cpu_time(&self, pid: u32, cpu_time_100ns: u64) {
        for process in lock(&self.processes).iter_mut() {
            if process.pid == pid {
                process.cpu_time_100ns = cpu_time_100ns;
            }
        }
    }

    /// Makes ending `pid` fail with `error`; the process stays running.
    pub fn fail_terminate(&self, pid: u32, error: PortError) {
        lock(&self.terminate_failures).insert(pid, error);
    }

    /// The settings of a running process, or `None` when there is none with that id.
    #[must_use]
    pub fn settings_of(&self, pid: u32) -> Option<ProcessSettings> {
        let known = lock(&self.processes).iter().any(|p| p.pid == pid);
        known.then(|| {
            lock(&self.tuned)
                .get(&pid)
                .copied()
                .unwrap_or(Self::DEFAULT_SETTINGS)
        })
    }

    /// Sets what a process holds.
    pub fn set_settings(&self, pid: u32, settings: ProcessSettings) {
        lock(&self.tuned).insert(pid, settings);
    }

    /// Makes reading the settings of `pid` fail with `error` (for example access denied).
    pub fn fail_settings(&self, pid: u32, error: PortError) {
        lock(&self.settings_failures).insert(pid, error);
    }

    /// Makes changing the settings of `pid` fail with `error`; nothing is changed then.
    pub fn fail_apply_settings(&self, pid: u32, error: PortError) {
        lock(&self.apply_failures).insert(pid, error);
    }

    /// Whether a process with this id and creation time runs.
    fn runs(&self, pid: u32, created_at_100ns: u64) -> bool {
        lock(&self.processes)
            .iter()
            .any(|p| p.pid == pid && p.created_at_100ns == created_at_100ns)
    }
}

impl Processes for FakeProcesses {
    fn list(&self) -> Result<Vec<ProcessInfo>, PortError> {
        let mut list = lock(&self.processes).clone();
        for process in &mut list {
            if let Some(shown) = lock(&self.listed_creation).get(&process.pid) {
                process.created_at_100ns = *shown;
            }
        }
        Ok(list)
    }

    fn terminate(&self, pid: u32, created_at_100ns: u64) -> Result<(), PortError> {
        if let Some(error) = lock(&self.terminate_failures).get(&pid) {
            return Err(error.clone());
        }
        let mut processes = lock(&self.processes);
        let before = processes.len();
        processes.retain(|p| !(p.pid == pid && p.created_at_100ns == created_at_100ns));
        if processes.len() == before {
            return Err(PortError::NotFound(format!("process {pid}")));
        }
        self.log.push(Event::ProcessTerminated(pid));
        Ok(())
    }

    fn settings(&self, pid: u32, created_at_100ns: u64) -> Result<ProcessSettings, PortError> {
        if !self.runs(pid, created_at_100ns) {
            return Err(PortError::NotFound(format!("process {pid}")));
        }
        if let Some(error) = lock(&self.settings_failures).get(&pid) {
            return Err(error.clone());
        }
        Ok(self.settings_of(pid).unwrap_or(Self::DEFAULT_SETTINGS))
    }

    fn apply_settings(
        &self,
        pid: u32,
        created_at_100ns: u64,
        change: &ProcessChange,
    ) -> Result<(), PortError> {
        let mut current = self.settings(pid, created_at_100ns)?;
        if let Some(error) = lock(&self.apply_failures).get(&pid) {
            return Err(error.clone());
        }
        if let Some(priority) = change.priority {
            current.priority = priority;
        }
        if let Some(mask) = change.affinity_mask {
            if mask == 0 || mask & !current.system_affinity_mask != 0 {
                return Err(PortError::InvalidInput("the affinity is not valid".into()));
            }
            current.affinity_mask = mask;
        }
        if let Some(throttling) = change.throttling {
            current.throttling = throttling;
        }
        lock(&self.tuned).insert(pid, current);
        self.log.push(Event::ProcessTuned(pid));
        Ok(())
    }
}

const GIB: u64 = 1 << 30;

/// In-memory [`Memory`]: 16 GiB with 8 GiB in use, 4 GiB standby, 1 GiB modified and 3 GiB
/// free. A purge moves pages between the lists the way Windows does (see [`FakeMemory::purge`]),
/// so a test can check a before and after.
#[derive(Debug)]
pub struct FakeMemory {
    lists: Mutex<MemoryLists>,
    purged: Mutex<Vec<MemoryArea>>,
    unavailable: Mutex<HashMap<MemoryArea, Availability>>,
    failures: Mutex<HashMap<MemoryArea, PortError>>,
    lists_fail: Mutex<Option<PortError>>,
    processes: Mutex<Vec<(String, bool)>>,
    trimmed: Mutex<Vec<String>>,
    log: EventLog,
}

impl FakeMemory {
    /// The default split, own event log.
    #[must_use]
    pub fn new() -> Self {
        Self::with_log(EventLog::default())
    }

    /// The default split, reporting to a shared event log.
    #[must_use]
    pub fn with_log(log: EventLog) -> Self {
        Self {
            lists: Mutex::new(MemoryLists::from_lists(16 * GIB, 4 * GIB, GIB, 3 * GIB)),
            purged: Mutex::new(Vec::new()),
            unavailable: Mutex::new(HashMap::new()),
            failures: Mutex::new(HashMap::new()),
            lists_fail: Mutex::new(None),
            processes: Mutex::new(vec![
                ("app.exe".to_owned(), true),
                ("editor.exe".to_owned(), true),
                ("service.exe".to_owned(), false),
            ]),
            trimmed: Mutex::new(Vec::new()),
            log,
        }
    }

    /// Replaces the running processes a working-set trim sees: each is an image name and whether
    /// the trim may open it (a process it cannot open is counted as skipped, like a protected one
    /// on a real machine). The default is two openable processes and one that is not.
    pub fn set_processes(&self, processes: &[(&str, bool)]) {
        *lock(&self.processes) = processes
            .iter()
            .map(|&(name, openable)| (name.to_owned(), openable))
            .collect();
    }

    /// Image names whose working set was trimmed so far, in order.
    #[must_use]
    pub fn trimmed_processes(&self) -> Vec<String> {
        lock(&self.trimmed).clone()
    }

    /// Areas purged so far, in order.
    #[must_use]
    pub fn purged(&self) -> Vec<MemoryArea> {
        lock(&self.purged).clone()
    }

    /// Reports `area` as not available, and makes a purge of it fail the way the real adapter
    /// does: [`PortError::AccessDenied`] for [`Availability::NeedsAdministrator`], otherwise
    /// [`PortError::Unsupported`].
    pub fn make_unavailable(&self, area: MemoryArea, why: Availability) {
        lock(&self.unavailable).insert(area, why);
    }

    /// Makes a purge of `area` fail with `error` (the area stays reported as available).
    pub fn fail_purge(&self, area: MemoryArea, error: PortError) {
        lock(&self.failures).insert(area, error);
    }

    /// Makes reading the lists fail with `error`.
    pub fn fail_lists(&self, error: PortError) {
        *lock(&self.lists_fail) = Some(error);
    }
}

impl Default for FakeMemory {
    fn default() -> Self {
        Self::new()
    }
}

impl Memory for FakeMemory {
    fn status(&self) -> Result<MemoryStatus, PortError> {
        let lists = *lock(&self.lists);
        Ok(MemoryStatus {
            total_physical_bytes: lists.total_bytes,
            available_physical_bytes: lists.standby_bytes + lists.free_bytes,
        })
    }

    fn lists(&self) -> Result<MemoryLists, PortError> {
        match &*lock(&self.lists_fail) {
            Some(error) => Err(error.clone()),
            None => Ok(*lock(&self.lists)),
        }
    }

    fn availability(&self, area: MemoryArea) -> Availability {
        lock(&self.unavailable)
            .get(&area)
            .copied()
            .unwrap_or(Availability::Available)
    }

    /// Working sets: 1 GiB of in-use pages becomes standby (two processes trimmed, one
    /// skipped). Standby: all standby pages become free. Low-priority standby: 1 GiB of the
    /// standby pages becomes free. Modified: all modified pages become standby. File cache:
    /// nothing moves.
    fn purge(&self, area: MemoryArea) -> Result<PurgeOutcome, PortError> {
        self.purge_protecting(area, &ExclusionList::new())
    }

    fn trim_working_sets(&self, protected: &ExclusionList) -> Result<PurgeOutcome, PortError> {
        self.purge_protecting(MemoryArea::WorkingSets, protected)
    }
}

impl FakeMemory {
    fn purge_protecting(
        &self,
        area: MemoryArea,
        protected: &ExclusionList,
    ) -> Result<PurgeOutcome, PortError> {
        match self.availability(area) {
            Availability::Available => {}
            Availability::NeedsAdministrator => {
                return Err(PortError::AccessDenied(format!(
                    "{} needs administrator rights",
                    area.name()
                )));
            }
            _ => {
                return Err(PortError::Unsupported(format!(
                    "{} is not supported",
                    area.name()
                )));
            }
        }
        if let Some(error) = lock(&self.failures).get(&area) {
            return Err(error.clone());
        }
        let mut lists = lock(&self.lists);
        let mut outcome = PurgeOutcome::default();
        match area {
            MemoryArea::WorkingSets => {
                let moved = GIB.min(lists.in_use_bytes);
                lists.in_use_bytes -= moved;
                lists.standby_bytes += moved;
                for (name, openable) in lock(&self.processes).iter() {
                    if protected.protects(name) {
                        outcome.processes_excluded += 1;
                    } else if *openable {
                        outcome.processes_trimmed += 1;
                        lock(&self.trimmed).push(name.clone());
                    } else {
                        outcome.processes_skipped += 1;
                    }
                }
            }
            MemoryArea::StandbyList => {
                lists.free_bytes += lists.standby_bytes;
                lists.standby_bytes = 0;
            }
            MemoryArea::StandbyListLowPriority => {
                let moved = GIB.min(lists.standby_bytes);
                lists.standby_bytes -= moved;
                lists.free_bytes += moved;
            }
            MemoryArea::ModifiedList => {
                lists.standby_bytes += lists.modified_bytes;
                lists.modified_bytes = 0;
            }
            _ => {}
        }
        drop(lists);
        lock(&self.purged).push(area);
        self.log.push(Event::MemoryPurged(area));
        Ok(outcome)
    }
}

#[derive(Debug, Default)]
struct CommandsState {
    toggles: Vec<Toggle>,
    exit_codes: HashMap<Command, i32>,
    leaks: HashMap<Command, i32>,
    failures: HashMap<Command, PortError>,
    ran: Vec<Command>,
}

/// A fake effect: `apply` turns it on, `revert` turns it off, `detect` exits
/// `0` when it is on and `1` when it is off.
#[derive(Debug)]
struct Toggle {
    detect: Command,
    apply: Command,
    revert: Command,
    on: bool,
}

/// In-memory [`Commands`]. Nothing is executed: commands act on registered
/// toggles, fixed exit codes or injected failures. A command that matches none
/// of them is [`PortError::NotFound`], like a missing program.
#[derive(Debug, Default)]
pub struct FakeCommands {
    state: Mutex<CommandsState>,
    log: EventLog,
}

impl FakeCommands {
    /// No commands known, own event log.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// No commands known, reporting to a shared event log.
    #[must_use]
    pub fn with_log(log: EventLog) -> Self {
        Self {
            log,
            ..Self::default()
        }
    }

    /// Registers a fake effect that `apply` turns on and `revert` turns off.
    /// `detect` exits `0` while it is on and `1` while it is off.
    pub fn add_toggle(&self, detect: Command, apply: Command, revert: Command, on: bool) {
        lock(&self.state).toggles.push(Toggle {
            detect,
            apply,
            revert,
            on,
        });
    }

    /// Whether the toggle whose detect command is `detect` is on.
    #[must_use]
    pub fn is_on(&self, detect: &Command) -> Option<bool> {
        lock(&self.state)
            .toggles
            .iter()
            .find(|t| t.detect == *detect)
            .map(|t| t.on)
    }

    /// Switches a toggle directly, without logging: a change made outside
    /// Volt. For arranging a test.
    pub fn set_on(&self, detect: &Command, on: bool) {
        if let Some(toggle) = lock(&self.state)
            .toggles
            .iter_mut()
            .find(|t| t.detect == *detect)
        {
            toggle.on = on;
        }
    }

    /// Makes `command` exit with `code` and do nothing else. Takes precedence
    /// over toggles.
    pub fn set_exit_code(&self, command: &Command, code: i32) {
        lock(&self.state).exit_codes.insert(command.clone(), code);
    }

    /// Makes `command` do its effect and still exit with `code`: a program
    /// that fails after it already changed something.
    pub fn leak(&self, command: &Command, code: i32) {
        lock(&self.state).leaks.insert(command.clone(), code);
    }

    /// Makes `command` fail with `error`, as if it could not be started.
    pub fn fail_on(&self, command: &Command, error: PortError) {
        lock(&self.state).failures.insert(command.clone(), error);
    }

    /// Every command run so far, detect commands included, in order.
    #[must_use]
    pub fn ran(&self) -> Vec<Command> {
        lock(&self.state).ran.clone()
    }
}

impl Commands for FakeCommands {
    fn run(&self, command: &Command) -> Result<CommandOutput, PortError> {
        let mut state = lock(&self.state);
        state.ran.push(command.clone());
        if let Some(error) = state.failures.get(command) {
            return Err(error.clone());
        }
        let exit_code = if let Some(code) = state.exit_codes.get(command).copied() {
            code
        } else if let Some(toggle) = state.toggles.iter().find(|t| t.detect == *command) {
            // Reading: not a side effect, so not in the event log.
            return Ok(CommandOutput {
                exit_code: i32::from(!toggle.on),
                output: String::new(),
            });
        } else {
            let leaked = state.leaks.get(command).copied();
            if let Some(toggle) = state.toggles.iter_mut().find(|t| t.apply == *command) {
                toggle.on = true;
            } else if let Some(toggle) = state.toggles.iter_mut().find(|t| t.revert == *command) {
                toggle.on = false;
            } else {
                return Err(PortError::NotFound(format!("program {}", command.program)));
            }
            leaked.unwrap_or(0)
        };
        self.log.push(Event::CommandRun(command.clone()));
        Ok(CommandOutput {
            exit_code,
            output: String::new(),
        })
    }
}

#[derive(Debug, Default)]
struct JournalState {
    next_entry: u64,
    next_batch: u64,
    batches: Vec<(BatchId, Option<String>)>,
    entries: Vec<JournalEntry>,
    fail_record: Option<PortError>,
    fail_set_state: Vec<(EntryState, PortError)>,
}

/// In-memory [`Journal`].
#[derive(Debug, Default)]
pub struct FakeJournal {
    state: Mutex<JournalState>,
    log: EventLog,
}

impl FakeJournal {
    /// Empty journal, own event log.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Empty journal, reporting to a shared event log.
    #[must_use]
    pub fn with_log(log: EventLog) -> Self {
        Self {
            log,
            ..Self::default()
        }
    }

    /// All entries, in write order.
    #[must_use]
    pub fn entries(&self) -> Vec<JournalEntry> {
        lock(&self.state).entries.clone()
    }

    /// Makes the next `record_pending` fail with `error`, once.
    pub fn fail_next_record(&self, error: PortError) {
        lock(&self.state).fail_record = Some(error);
    }

    /// Makes the next `set_state` to `state` fail with `error`, once.
    pub fn fail_next_set_state(&self, state: EntryState, error: PortError) {
        lock(&self.state).fail_set_state.push((state, error));
    }
}

impl Journal for FakeJournal {
    fn new_batch(&self, label: Option<&str>) -> Result<BatchId, PortError> {
        let mut state = lock(&self.state);
        state.next_batch += 1;
        let batch = BatchId(state.next_batch);
        state.batches.push((batch, label.map(str::to_owned)));
        Ok(batch)
    }

    fn batches_with_label(&self, label: &str) -> Result<Vec<BatchId>, PortError> {
        Ok(lock(&self.state)
            .batches
            .iter()
            .filter(|(_, l)| l.as_deref() == Some(label))
            .map(|(id, _)| *id)
            .collect())
    }

    fn batch_label(&self, batch: BatchId) -> Result<Option<String>, PortError> {
        Ok(lock(&self.state)
            .batches
            .iter()
            .find(|(id, _)| *id == batch)
            .and_then(|(_, label)| label.clone()))
    }

    fn record_pending(&self, entry: NewEntry) -> Result<EntryId, PortError> {
        let mut state = lock(&self.state);
        if let Some(error) = state.fail_record.take() {
            return Err(error);
        }
        state.next_entry += 1;
        let id = EntryId(state.next_entry);
        state.entries.push(JournalEntry {
            id,
            batch: entry.batch,
            timestamp: entry.timestamp,
            source: entry.source,
            state: EntryState::Pending,
            operation: entry.operation,
            prior: entry.prior,
            error: None,
        });
        self.log.push(Event::JournalPending(id));
        Ok(id)
    }

    fn set_state(
        &self,
        id: EntryId,
        new_state: EntryState,
        error: Option<String>,
    ) -> Result<(), PortError> {
        let mut state = lock(&self.state);
        if let Some(pos) = state
            .fail_set_state
            .iter()
            .position(|(s, _)| *s == new_state)
        {
            return Err(state.fail_set_state.remove(pos).1);
        }
        let entry = state
            .entries
            .iter_mut()
            .find(|e| e.id == id)
            .ok_or_else(|| PortError::NotFound(format!("journal entry {id}")))?;
        entry.state = new_state;
        entry.error = error;
        self.log.push(Event::JournalState(id, new_state));
        Ok(())
    }

    fn entry(&self, id: EntryId) -> Result<Option<JournalEntry>, PortError> {
        Ok(lock(&self.state)
            .entries
            .iter()
            .find(|e| e.id == id)
            .cloned())
    }

    fn batch_entries(&self, batch: BatchId) -> Result<Vec<JournalEntry>, PortError> {
        Ok(lock(&self.state)
            .entries
            .iter()
            .filter(|e| e.batch == batch)
            .cloned()
            .collect())
    }

    fn entries_in_state(&self, state: EntryState) -> Result<Vec<JournalEntry>, PortError> {
        Ok(lock(&self.state)
            .entries
            .iter()
            .filter(|e| e.state == state)
            .cloned()
            .collect())
    }
}

#[derive(Debug, Default)]
struct ActivityState {
    records: Vec<ActivityRecord>,
    fail_with: Option<PortError>,
    protected: Vec<crate::journal::EntryId>,
}

/// In-memory [`ActivityLog`].
#[derive(Debug, Default)]
pub struct FakeActivityLog {
    state: Mutex<ActivityState>,
}

impl FakeActivityLog {
    /// Empty log.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// All records, in write order (oldest first).
    #[must_use]
    pub fn records(&self) -> Vec<ActivityRecord> {
        lock(&self.state).records.clone()
    }

    /// Makes every following `record` fail with `error`, or stops doing so
    /// with `None`.
    pub fn fail_records_with(&self, error: Option<PortError>) {
        lock(&self.state).fail_with = error;
    }

    /// Marks a journal entry as one that can still be undone: `prune` and `clear` keep the
    /// records linked to it (the real adapter reads this from the journal).
    pub fn protect_entry(&self, entry: crate::journal::EntryId) {
        lock(&self.state).protected.push(entry);
    }
}

impl ActivityLog for FakeActivityLog {
    fn record(&self, activity: NewActivity) -> Result<ActivityId, PortError> {
        let mut state = lock(&self.state);
        if let Some(error) = &state.fail_with {
            return Err(error.clone());
        }
        let id = ActivityId(state.records.len() as u64 + 1);
        state.records.push(ActivityRecord {
            id,
            timestamp: activity.timestamp,
            source: activity.source,
            outcome: activity.outcome,
            summary: activity.summary,
            batch: activity.batch,
            entry: activity.entry,
            label: activity.label,
            phrase: activity.phrase,
        });
        Ok(id)
    }

    fn list(
        &self,
        limit: usize,
        before: Option<ActivityId>,
    ) -> Result<Vec<ActivityRecord>, PortError> {
        Ok(lock(&self.state)
            .records
            .iter()
            .rev()
            .filter(|record| before.is_none_or(|cursor| record.id < cursor))
            .take(limit)
            .cloned()
            .collect())
    }

    fn list_after(
        &self,
        after: Option<ActivityId>,
        limit: usize,
    ) -> Result<Vec<ActivityRecord>, PortError> {
        Ok(lock(&self.state)
            .records
            .iter()
            .filter(|record| after.is_none_or(|cursor| record.id > cursor))
            .take(limit)
            .cloned()
            .collect())
    }

    fn prune(&self, retention: Retention, now: Timestamp) -> Result<u64, PortError> {
        let mut state = lock(&self.state);
        let cutoff = now.as_unix_millis().saturating_sub(retention.max_age_ms);
        let total = state.records.len();
        let protected = state.protected.clone();
        let mut index = 0;
        let before = total;
        state.records.retain(|record| {
            index += 1;
            let rank_from_newest = total - index;
            let expired = record.timestamp.as_unix_millis() < cutoff
                || rank_from_newest >= retention.max_records;
            let kept = record.entry.is_some_and(|e| protected.contains(&e));
            !expired || kept
        });
        Ok((before - state.records.len()) as u64)
    }

    fn clear(&self) -> Result<u64, PortError> {
        let mut state = lock(&self.state);
        let protected = state.protected.clone();
        let before = state.records.len();
        state
            .records
            .retain(|record| record.entry.is_some_and(|e| protected.contains(&e)));
        Ok((before - state.records.len()) as u64)
    }
}

/// In-memory [`Settings`].
#[derive(Debug, Default)]
pub struct FakeSettings {
    values: Mutex<HashMap<String, serde_json::Value>>,
}

impl FakeSettings {
    /// No settings.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl Settings for FakeSettings {
    fn get(&self, key: &str) -> Result<Option<serde_json::Value>, PortError> {
        Ok(lock(&self.values).get(key).cloned())
    }

    fn set(&self, key: &str, value: &serde_json::Value) -> Result<(), PortError> {
        lock(&self.values).insert(key.to_owned(), value.clone());
        Ok(())
    }

    fn remove(&self, key: &str) -> Result<bool, PortError> {
        Ok(lock(&self.values).remove(key).is_some())
    }
}

/// Deterministic [`Clock`]: returns a start time, then 1 ms more on every call.
#[derive(Debug)]
pub struct ManualClock(AtomicI64);

impl ManualClock {
    /// A clock whose first reading is `start_millis`.
    #[must_use]
    pub fn new(start_millis: i64) -> Self {
        Self(AtomicI64::new(start_millis))
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Timestamp {
        Timestamp::from_unix_millis(self.0.fetch_add(1, Ordering::SeqCst))
    }
}

/// All fakes wired to one [`EventLog`], with an [`Executor`] over them.
#[derive(Debug)]
pub struct FakeSystem {
    /// Registry fake.
    pub registry: Arc<FakeRegistry>,
    /// File system fake.
    pub file_system: Arc<FakeFileSystem>,
    /// Services fake.
    pub services: Arc<FakeServices>,
    /// Processes fake.
    pub processes: Arc<FakeProcesses>,
    /// Memory fake.
    pub memory: Arc<FakeMemory>,
    /// Commands fake.
    pub commands: Arc<FakeCommands>,
    /// Journal fake.
    pub journal: Arc<FakeJournal>,
    /// Activity log fake; [`FakeSystem::executor_with_activity`] writes to it.
    pub activity: Arc<FakeActivityLog>,
    /// Clock fake, starting at 1000 ms.
    pub clock: Arc<ManualClock>,
    /// Ordered record of every side effect of the fakes above.
    pub log: EventLog,
}

impl FakeSystem {
    /// A fresh, empty fake system.
    #[must_use]
    pub fn new() -> Self {
        let log = EventLog::default();
        Self {
            registry: Arc::new(FakeRegistry::with_log(log.clone())),
            file_system: Arc::new(FakeFileSystem::with_log(log.clone())),
            services: Arc::new(FakeServices::with_log(log.clone())),
            processes: Arc::new(FakeProcesses::with_log(log.clone())),
            memory: Arc::new(FakeMemory::with_log(log.clone())),
            commands: Arc::new(FakeCommands::with_log(log.clone())),
            journal: Arc::new(FakeJournal::with_log(log.clone())),
            activity: Arc::new(FakeActivityLog::new()),
            clock: Arc::new(ManualClock::new(1000)),
            log,
        }
    }

    /// The fakes as a [`Ports`] bundle.
    #[must_use]
    pub fn ports(&self) -> Ports {
        Ports {
            registry: self.registry.clone(),
            file_system: self.file_system.clone(),
            services: self.services.clone(),
            processes: self.processes.clone(),
            memory: self.memory.clone(),
            commands: self.commands.clone(),
        }
    }

    /// An executor over these fakes, without an activity log.
    #[must_use]
    pub fn executor(&self) -> Executor {
        Executor::new(self.ports(), self.journal.clone(), self.clock.clone())
    }

    /// Like [`FakeSystem::executor`], writing to the [`FakeSystem::activity`]
    /// fake. Activity records take their times from the shared clock, so use
    /// this only where the exact clock readings do not matter.
    #[must_use]
    pub fn executor_with_activity(&self) -> Executor {
        self.executor().with_activity_log(self.activity.clone())
    }
}

impl Default for FakeSystem {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::Source;
    use crate::operation::{Operation, PriorState};

    #[test]
    fn registry_is_case_insensitive_and_delete_of_missing_value_is_not_found() {
        let registry = FakeRegistry::new();
        let upper = RegistryValueRef::new(RegistryHive::CurrentUser, r"Software\VoltTest", "Flag");
        let lower = RegistryValueRef::new(RegistryHive::CurrentUser, r"software\volttest", "flag");
        registry
            .write_value(&upper, &RegistryValue::Dword(1))
            .expect("write");
        assert_eq!(
            registry.read_value(&lower),
            Ok(Some(RegistryValue::Dword(1)))
        );
        // The 32-bit view is a different key.
        assert_eq!(
            registry.read_value(&upper.clone().with_view(RegistryView::Bit32)),
            Ok(None)
        );
        registry.delete_value(&lower).expect("delete");
        assert!(matches!(
            registry.delete_value(&upper),
            Err(PortError::NotFound(_))
        ));
    }

    #[test]
    fn services_report_unknown_services_as_not_found() {
        let services = FakeServices::new();
        services.add("Spooler", ServiceStartType::Automatic);
        services
            .set_start_type("Spooler", ServiceStartType::Manual)
            .expect("set");
        assert_eq!(services.start_type("Spooler"), Ok(ServiceStartType::Manual));
        assert!(matches!(
            services.start_type("Nope"),
            Err(PortError::NotFound(_))
        ));
        assert!(matches!(
            services.set_start_type("Nope", ServiceStartType::Disabled),
            Err(PortError::NotFound(_))
        ));
    }

    #[test]
    fn file_system_trashes_files_and_records_them() {
        let fs = FakeFileSystem::new();
        fs.add_file("C:/Temp/a.tmp", 42);
        let path = Path::new("C:/Temp/a.tmp");
        assert_eq!(
            fs.stat(path),
            Ok(Some(FileStat {
                size_bytes: 42,
                is_dir: false
            }))
        );
        let root = Path::new("C:/Temp");
        assert!(matches!(
            fs.move_to_trash(Path::new("C:/Other"), path),
            Err(PortError::InvalidInput(_))
        ));
        fs.move_to_trash(root, path).expect("trash");
        assert_eq!(fs.stat(path), Ok(None));
        assert_eq!(fs.trashed(), [path.to_path_buf()]);
        assert!(matches!(
            fs.move_to_trash(root, path),
            Err(PortError::NotFound(_))
        ));
    }

    #[test]
    fn file_system_lists_directories_sorted_and_describes_entries_without_following_links() {
        let fs = FakeFileSystem::new();
        let modified = Timestamp::from_unix_millis(5);
        fs.add_file_at("C:/D/b.txt", 7, modified);
        fs.add_file("C:/D/a.txt", 1);
        fs.add_dir("C:/D/sub", None);
        fs.add_file("C:/D/link/inside.txt", 3);
        fs.add_link("C:/D/link");

        let listing = fs.read_dir(Path::new("C:/D")).expect("list");
        let names: Vec<_> = listing
            .iter()
            .map(|e| e.name.to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["a.txt", "b.txt", "link", "sub"]);
        let kind = |name: &str| {
            listing
                .iter()
                .find(|e| e.name == name)
                .and_then(|e| e.info.as_ref().ok())
                .map(|i| i.kind)
        };
        assert_eq!(kind("link"), Some(EntryKind::ReparsePoint));
        assert_eq!(kind("sub"), Some(EntryKind::Directory));
        assert_eq!(
            fs.inspect(Path::new("C:/D/b.txt")),
            Ok(Some(EntryInfo {
                kind: EntryKind::File,
                size_bytes: 7,
                modified: Some(modified),
                system: false,
            }))
        );
        assert_eq!(fs.inspect(Path::new("C:/D/missing")), Ok(None));
        // A link is not a directory to list, and neither is a file.
        assert!(matches!(
            fs.read_dir(Path::new("C:/D/link")),
            Err(PortError::InvalidInput(_))
        ));
        assert!(matches!(
            fs.read_dir(Path::new("C:/D/a.txt")),
            Err(PortError::InvalidInput(_))
        ));
        assert!(matches!(
            fs.read_dir(Path::new("C:/Nope")),
            Err(PortError::NotFound(_))
        ));
    }

    #[test]
    fn file_system_trash_refuses_the_root_parent_components_and_paths_behind_links() {
        let fs = FakeFileSystem::new();
        fs.add_file("C:/R/real.txt", 1);
        fs.add_file("C:/R/link/victim.txt", 1);
        fs.add_link("C:/R/link");
        fs.add_file("C:/Other/outside.txt", 1);
        let root = Path::new("C:/R");
        for path in [
            "C:/R",
            "C:/R/link",
            "C:/R/link/victim.txt",
            "C:/R/../Other/outside.txt",
            "C:/Other/outside.txt",
        ] {
            assert!(
                matches!(
                    fs.move_to_trash(root, Path::new(path)),
                    Err(PortError::InvalidInput(_))
                ),
                "{path} must be refused"
            );
        }
        assert!(fs.trashed().is_empty());
        fs.move_to_trash(root, Path::new("C:/R/real.txt"))
            .expect("trash");
        assert_eq!(fs.trashed().len(), 1);
    }

    #[test]
    fn processes_list_and_terminate() {
        let processes = FakeProcesses::new();
        processes.add(10, "a.exe", 100);
        processes.add(11, "b.exe", 200);
        processes.terminate(10, 0).expect("terminate");
        assert_eq!(processes.list().expect("list").len(), 1);
        assert!(matches!(
            processes.terminate(10, 0),
            Err(PortError::NotFound(_))
        ));
    }

    #[test]
    fn memory_reports_status_and_records_purges() {
        let memory = FakeMemory::new();
        let status = memory.status().expect("status");
        assert!(status.available_physical_bytes < status.total_physical_bytes);
        memory.purge(MemoryArea::StandbyList).expect("purge");
        assert_eq!(memory.lists().expect("lists").standby_bytes, 0);
        assert_eq!(memory.purged(), [MemoryArea::StandbyList]);
    }

    #[test]
    fn settings_set_get_remove() {
        let settings = FakeSettings::new();
        assert_eq!(settings.get("a"), Ok(None));
        settings
            .set("a", &serde_json::json!({"x": 1}))
            .expect("set");
        assert_eq!(settings.get("a"), Ok(Some(serde_json::json!({"x": 1}))));
        assert_eq!(settings.remove("a"), Ok(true));
        assert_eq!(settings.remove("a"), Ok(false));
    }

    #[test]
    fn journal_assigns_increasing_ids_and_filters_by_batch() {
        let journal = FakeJournal::new();
        let (b1, b2) = (
            journal.new_batch(None).expect("batch"),
            journal.new_batch(Some("tweak-a")).expect("batch"),
        );
        assert_ne!(b1, b2);
        let b3 = journal.new_batch(Some("tweak-a")).expect("batch");
        assert_eq!(journal.batches_with_label("tweak-a"), Ok(vec![b2, b3]));
        assert_eq!(journal.batches_with_label("other"), Ok(vec![]));
        let new_entry = |batch| NewEntry {
            batch,
            timestamp: Timestamp::from_unix_millis(1),
            source: Source::Manual,
            operation: Operation::DeleteRegistryValue {
                target: RegistryValueRef::new(RegistryHive::CurrentUser, r"Software\X", "v"),
            },
            prior: PriorState::RegistryValue(None),
        };
        let first = journal.record_pending(new_entry(b1)).expect("record");
        let second = journal.record_pending(new_entry(b2)).expect("record");
        assert!(first < second);
        assert_eq!(journal.batch_entries(b2).expect("batch").len(), 1);
        assert!(matches!(
            journal.set_state(EntryId(99), EntryState::Applied, None),
            Err(PortError::NotFound(_))
        ));
    }
}
