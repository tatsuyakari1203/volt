//! In-memory fakes of the [`FilePublisher`], [`ShortcutResolver`], [`LogonTasks`] and
//! [`AutoStartServices`] ports.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};

use super::{
    AutoStartService, AutoStartServices, FilePublisher, LogonTask, LogonTasks, ShortcutResolver,
    ShortcutTarget,
};
use crate::error::PortError;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A [`FilePublisher`] whose answers are set by the test. A program that was
/// not set has no publisher.
#[derive(Debug, Default)]
pub struct FakePublisher {
    answers: Mutex<HashMap<String, Result<Option<String>, PortError>>>,
    asked: Mutex<Vec<String>>,
}

impl FakePublisher {
    /// Knows no publishers.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Says who published `executable` (compared ignoring case).
    pub fn set(&self, executable: &str, publisher: &str) {
        lock(&self.answers).insert(executable.to_lowercase(), Ok(Some(publisher.to_owned())));
    }

    /// Makes asking about `executable` (compared ignoring case) fail.
    pub fn fail_on(&self, executable: &str, error: PortError) {
        lock(&self.answers).insert(executable.to_lowercase(), Err(error));
    }

    /// The programs asked about so far, in order.
    #[must_use]
    pub fn asked(&self) -> Vec<String> {
        lock(&self.asked).clone()
    }
}

impl FilePublisher for FakePublisher {
    fn publisher(&self, executable: &str) -> Result<Option<String>, PortError> {
        lock(&self.asked).push(executable.to_owned());
        lock(&self.answers)
            .get(&executable.to_lowercase())
            .cloned()
            .unwrap_or(Ok(None))
    }
}

/// A [`ShortcutResolver`] whose answers are set by the test, by file name
/// (compared ignoring case). An unknown shortcut has no target.
#[derive(Debug, Default)]
pub struct FakeShortcuts {
    answers: Mutex<HashMap<String, Result<Option<ShortcutTarget>, PortError>>>,
}

impl FakeShortcuts {
    /// Knows no shortcuts.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Says where the shortcut file named `file_name` points.
    pub fn set(&self, file_name: &str, target: &str, arguments: &str) {
        lock(&self.answers).insert(
            file_name.to_lowercase(),
            Ok(Some(ShortcutTarget {
                target: target.to_owned(),
                arguments: arguments.to_owned(),
            })),
        );
    }

    /// Makes reading the shortcut file named `file_name` fail.
    pub fn fail_on(&self, file_name: &str, error: PortError) {
        lock(&self.answers).insert(file_name.to_lowercase(), Err(error));
    }
}

impl ShortcutResolver for FakeShortcuts {
    fn resolve(&self, shortcut: &Path) -> Result<Option<ShortcutTarget>, PortError> {
        let name = shortcut
            .file_name()
            .map(|name| name.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        lock(&self.answers).get(&name).cloned().unwrap_or(Ok(None))
    }
}

/// A [`LogonTasks`] source whose answer is set by the test.
#[derive(Debug)]
pub struct FakeLogonTasks {
    answer: Mutex<Result<Vec<LogonTask>, PortError>>,
    calls: std::sync::atomic::AtomicUsize,
}

impl FakeLogonTasks {
    /// A source with these tasks.
    #[must_use]
    pub fn new(tasks: Vec<LogonTask>) -> Self {
        Self {
            answer: Mutex::new(Ok(tasks)),
            calls: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// A source that cannot be read.
    #[must_use]
    pub fn failing(error: PortError) -> Self {
        Self {
            answer: Mutex::new(Err(error)),
            calls: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// How many times the tasks were asked for.
    #[must_use]
    pub fn calls(&self) -> usize {
        self.calls.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl LogonTasks for FakeLogonTasks {
    fn logon_tasks(&self) -> Result<Vec<LogonTask>, PortError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        lock(&self.answer).clone()
    }
}

/// An [`AutoStartServices`] source whose answer is set by the test.
#[derive(Debug)]
pub struct FakeAutoStartServices {
    answer: Mutex<Result<Vec<AutoStartService>, PortError>>,
}

impl FakeAutoStartServices {
    /// A source with these services.
    #[must_use]
    pub fn new(services: Vec<AutoStartService>) -> Self {
        Self {
            answer: Mutex::new(Ok(services)),
        }
    }

    /// A source that cannot be read.
    #[must_use]
    pub fn failing(error: PortError) -> Self {
        Self {
            answer: Mutex::new(Err(error)),
        }
    }
}

impl AutoStartServices for FakeAutoStartServices {
    fn auto_start_services(&self) -> Result<Vec<AutoStartService>, PortError> {
        lock(&self.answer).clone()
    }
}
