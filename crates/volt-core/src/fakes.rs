//! In-memory fakes of every port, for tests that must not touch the real system.
//!
//! Available to this crate's own tests and to other crates through the `fakes`
//! cargo feature. [`FakeSystem`] wires them together with a shared
//! [`EventLog`], so a test can prove the order of effects (for example that the
//! journal entry was written before the registry was mutated).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::error::PortError;
use crate::executor::Executor;
use crate::journal::{BatchId, EntryId, EntryState, Journal, JournalEntry, NewEntry};
use crate::ports::{
    FileStat, FileSystem, Memory, MemoryArea, MemoryStatus, Ports, ProcessInfo, Processes, Services,
};
use crate::registry::{Registry, RegistryHive, RegistryValue, RegistryValueRef, RegistryView};
use crate::service::ServiceStartType;
use crate::settings::Settings;
use crate::time::{Clock, Timestamp};

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
    /// A service start type was changed.
    ServiceSetStartType(String, ServiceStartType),
    /// A path was moved to the trash.
    FileTrashed(PathBuf),
    /// A process was terminated.
    ProcessTerminated(u32),
    /// A memory area was purged.
    MemoryPurged(MemoryArea),
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

/// In-memory [`Registry`].
#[derive(Debug, Default)]
pub struct FakeRegistry {
    values: Mutex<HashMap<Key, RegistryValue>>,
    failures: Mutex<HashMap<Key, PortError>>,
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
        lock(&self.values).insert(Key::of(target), value);
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

    fn check_failure(&self, key: &Key) -> Result<(), PortError> {
        match lock(&self.failures).get(key) {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        }
    }
}

impl Registry for FakeRegistry {
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
        lock(&self.values).insert(key, data.clone());
        self.log.push(Event::RegistryWrite(target.clone()));
        Ok(())
    }

    fn delete_value(&self, target: &RegistryValueRef) -> Result<(), PortError> {
        let key = Key::of(target);
        self.check_failure(&key)?;
        if lock(&self.values).remove(&key).is_none() {
            return Err(PortError::NotFound(target.to_string()));
        }
        self.log.push(Event::RegistryDelete(target.clone()));
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

/// In-memory [`FileSystem`].
#[derive(Debug, Default)]
pub struct FakeFileSystem {
    entries: Mutex<HashMap<PathBuf, FileStat>>,
    trashed: Mutex<Vec<PathBuf>>,
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

    /// Adds a file of `size_bytes`.
    pub fn add_file(&self, path: impl Into<PathBuf>, size_bytes: u64) {
        lock(&self.entries).insert(
            path.into(),
            FileStat {
                size_bytes,
                is_dir: false,
            },
        );
    }

    /// Paths moved to the trash so far, in order.
    #[must_use]
    pub fn trashed(&self) -> Vec<PathBuf> {
        lock(&self.trashed).clone()
    }
}

impl FileSystem for FakeFileSystem {
    fn stat(&self, path: &Path) -> Result<Option<FileStat>, PortError> {
        Ok(lock(&self.entries).get(path).copied())
    }

    fn move_to_trash(&self, root: &Path, path: &Path) -> Result<(), PortError> {
        if !path.starts_with(root) {
            return Err(PortError::InvalidInput(format!(
                "{} is outside {}",
                path.display(),
                root.display()
            )));
        }
        if lock(&self.entries).remove(path).is_none() {
            return Err(PortError::NotFound(path.display().to_string()));
        }
        lock(&self.trashed).push(path.to_path_buf());
        self.log.push(Event::FileTrashed(path.to_path_buf()));
        Ok(())
    }
}

/// In-memory [`Processes`].
#[derive(Debug, Default)]
pub struct FakeProcesses {
    processes: Mutex<Vec<ProcessInfo>>,
    log: EventLog,
}

impl FakeProcesses {
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

    /// Adds a running process.
    pub fn add(&self, pid: u32, name: &str, working_set_bytes: u64) {
        lock(&self.processes).push(ProcessInfo {
            pid,
            name: name.to_owned(),
            working_set_bytes,
        });
    }
}

impl Processes for FakeProcesses {
    fn list(&self) -> Result<Vec<ProcessInfo>, PortError> {
        Ok(lock(&self.processes).clone())
    }

    fn terminate(&self, pid: u32) -> Result<(), PortError> {
        let mut processes = lock(&self.processes);
        let before = processes.len();
        processes.retain(|p| p.pid != pid);
        if processes.len() == before {
            return Err(PortError::NotFound(format!("process {pid}")));
        }
        self.log.push(Event::ProcessTerminated(pid));
        Ok(())
    }
}

/// In-memory [`Memory`].
#[derive(Debug)]
pub struct FakeMemory {
    status: Mutex<MemoryStatus>,
    purged: Mutex<Vec<MemoryArea>>,
    log: EventLog,
}

impl FakeMemory {
    /// 16 GiB total, 8 GiB available, own event log.
    #[must_use]
    pub fn new() -> Self {
        Self::with_log(EventLog::default())
    }

    /// 16 GiB total, 8 GiB available, reporting to a shared event log.
    #[must_use]
    pub fn with_log(log: EventLog) -> Self {
        const GIB: u64 = 1 << 30;
        Self {
            status: Mutex::new(MemoryStatus {
                total_physical_bytes: 16 * GIB,
                available_physical_bytes: 8 * GIB,
            }),
            purged: Mutex::new(Vec::new()),
            log,
        }
    }

    /// Areas purged so far, in order.
    #[must_use]
    pub fn purged(&self) -> Vec<MemoryArea> {
        lock(&self.purged).clone()
    }
}

impl Default for FakeMemory {
    fn default() -> Self {
        Self::new()
    }
}

impl Memory for FakeMemory {
    fn status(&self) -> Result<MemoryStatus, PortError> {
        Ok(*lock(&self.status))
    }

    fn purge(&self, area: MemoryArea) -> Result<(), PortError> {
        lock(&self.purged).push(area);
        self.log.push(Event::MemoryPurged(area));
        Ok(())
    }
}

#[derive(Debug, Default)]
struct JournalState {
    next_entry: u64,
    next_batch: u64,
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
    fn new_batch(&self) -> Result<BatchId, PortError> {
        let mut state = lock(&self.state);
        state.next_batch += 1;
        Ok(BatchId(state.next_batch))
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
    /// Journal fake.
    pub journal: Arc<FakeJournal>,
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
            journal: Arc::new(FakeJournal::with_log(log.clone())),
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
        }
    }

    /// An executor over these fakes.
    #[must_use]
    pub fn executor(&self) -> Executor {
        Executor::new(self.ports(), self.journal.clone(), self.clock.clone())
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
    fn processes_list_and_terminate() {
        let processes = FakeProcesses::new();
        processes.add(10, "a.exe", 100);
        processes.add(11, "b.exe", 200);
        processes.terminate(10).expect("terminate");
        assert_eq!(processes.list().expect("list").len(), 1);
        assert!(matches!(
            processes.terminate(10),
            Err(PortError::NotFound(_))
        ));
    }

    #[test]
    fn memory_reports_status_and_records_purges() {
        let memory = FakeMemory::new();
        let status = memory.status().expect("status");
        assert!(status.available_physical_bytes < status.total_physical_bytes);
        memory.purge(MemoryArea::StandbyList).expect("purge");
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
            journal.new_batch().expect("batch"),
            journal.new_batch().expect("batch"),
        );
        assert_ne!(b1, b2);
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
