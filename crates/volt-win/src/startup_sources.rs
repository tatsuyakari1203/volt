//! Read-only adapters for the startup manager: where a `.lnk` shortcut points
//! (the shell link COM object) and which scheduled tasks run at logon (the Task
//! Scheduler COM service). Nothing here runs, creates, changes or deletes a
//! shortcut or a task.

use std::path::Path;

use volt_core::{LogonTask, LogonTasks, PortError, ShortcutResolver, ShortcutTarget};
use windows::Win32::Foundation::{MAX_PATH, RPC_E_CHANGED_MODE, VARIANT_BOOL};
use windows::Win32::Storage::FileSystem::WIN32_FIND_DATAW;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
    IPersistFile, STGM_READ,
};
use windows::Win32::System::TaskScheduler::{
    IExecAction, IRegisteredTask, ITaskFolder, ITaskService, TASK_ACTION_EXEC, TASK_ACTION_TYPE,
    TASK_ENUM_HIDDEN, TASK_TRIGGER_LOGON, TASK_TRIGGER_TYPE2, TaskScheduler,
};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
use windows::core::{BSTR, Interface, PCWSTR};

use crate::error::from_hresult;
use crate::wide::to_wide_nul;

/// How deep the task folders are followed, and how many tasks are looked at:
/// bounds against a pathological library.
const MAX_FOLDER_DEPTH: u32 = 8;
const MAX_TASKS: usize = 5000;
/// `GetPath` flag: return the stored path without searching for a moved target.
const SLGP_RAWPATH: u32 = 4;

/// Keeps COM initialized on the calling thread for as long as it lives.
struct ComScope {
    uninitialize: bool,
}

impl ComScope {
    fn enter() -> Result<Self, PortError> {
        // SAFETY: plain COM initialization of the calling thread; balanced by
        // `CoUninitialize` in `Drop` when (and only when) it succeeded.
        let result = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if result.is_ok() {
            Ok(Self { uninitialize: true })
        } else if result == RPC_E_CHANGED_MODE {
            // The thread already runs COM in another mode: use it as it is.
            Ok(Self {
                uninitialize: false,
            })
        } else {
            Err(from_hresult(result.0, "initializing COM"))
        }
    }
}

impl Drop for ComScope {
    fn drop(&mut self) {
        if self.uninitialize {
            // SAFETY: balances the successful `CoInitializeEx` of `enter`.
            unsafe { CoUninitialize() };
        }
    }
}

fn com_error(error: &windows::core::Error, context: &str) -> PortError {
    from_hresult(error.code().0, context)
}

/// Resolves `.lnk` shortcuts with the shell link object. Read-only: the file
/// is loaded for reading and its target is never opened or run.
#[derive(Debug, Default, Clone, Copy)]
pub struct WinShortcuts;

impl WinShortcuts {
    /// A shortcut reader.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

fn text(buffer: &[u16]) -> String {
    let end = buffer
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}

impl ShortcutResolver for WinShortcuts {
    fn resolve(&self, shortcut: &Path) -> Result<Option<ShortcutTarget>, PortError> {
        let wide = to_wide_nul(&shortcut.to_string_lossy(), "shortcut path")?;
        let _com = ComScope::enter()?;
        // SAFETY: creates the in-process shell link object; no pointers passed.
        let link: IShellLinkW = unsafe { CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER) }
            .map_err(|e| com_error(&e, "creating the shell link object"))?;
        let file: IPersistFile = link
            .cast()
            .map_err(|e| com_error(&e, "reading a shortcut"))?;
        // SAFETY: `wide` is a NUL-terminated UTF-16 path that outlives the call;
        // the file is opened for reading only.
        unsafe { file.Load(PCWSTR(wide.as_ptr()), STGM_READ) }
            .map_err(|e| com_error(&e, "reading a shortcut"))?;

        let mut path = [0_u16; MAX_PATH as usize];
        let mut find = WIN32_FIND_DATAW::default();
        // SAFETY: `path` is a writable buffer the call fills with at most its
        // length; `find` is a valid out-structure; the flag asks for the stored
        // path without resolving or searching.
        unsafe { link.GetPath(&mut path, &raw mut find, SLGP_RAWPATH) }
            .map_err(|e| com_error(&e, "reading a shortcut"))?;
        let target = text(&path);
        if target.is_empty() {
            return Ok(None);
        }
        let mut buffer = [0_u16; 2048];
        // SAFETY: `buffer` is a writable buffer the call fills within its length.
        let arguments = match unsafe { link.GetArguments(&mut buffer) } {
            Ok(()) => text(&buffer),
            Err(_) => String::new(),
        };
        Ok(Some(ShortcutTarget { target, arguments }))
    }
}

/// Lists logon scheduled tasks with the Task Scheduler service. Read-only: it
/// only reads definitions, and a task the current user cannot read is left out.
#[derive(Debug, Default, Clone, Copy)]
pub struct WinLogonTasks;

impl WinLogonTasks {
    /// A scheduled task reader.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl LogonTasks for WinLogonTasks {
    fn logon_tasks(&self) -> Result<Vec<LogonTask>, PortError> {
        let _com = ComScope::enter()?;
        // SAFETY: creates the in-process Task Scheduler client object.
        let service: ITaskService =
            unsafe { CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER) }
                .map_err(|e| com_error(&e, "creating the Task Scheduler client"))?;
        let empty = VARIANT::default();
        // SAFETY: four valid empty VARIANTs: connect to the local service as
        // the current user.
        unsafe { service.Connect(&empty, &empty, &empty, &empty) }
            .map_err(|e| com_error(&e, "connecting to the Task Scheduler"))?;
        // SAFETY: the `BSTR` argument is owned for the duration of the call.
        let root = unsafe { service.GetFolder(&BSTR::from("\\")) }
            .map_err(|e| com_error(&e, "opening the task library"))?;
        let mut found = Vec::new();
        collect(&root, 0, &mut found);
        Ok(found)
    }
}

/// Adds the logon tasks of `folder` and its sub-folders. Anything unreadable
/// is skipped.
fn collect(folder: &ITaskFolder, depth: u32, found: &mut Vec<LogonTask>) {
    // SAFETY: read-only COM calls on live interfaces; hidden tasks included.
    if let Ok(tasks) = unsafe { folder.GetTasks(TASK_ENUM_HIDDEN.0) }
        && let Ok(count) =
            // SAFETY: read-only COM call on a live interface.
            unsafe { tasks.Count() }
    {
        for index in 1..=count {
            if found.len() >= MAX_TASKS {
                return;
            }
            // SAFETY: a 1-based index within `Count`.
            let Ok(task) = (unsafe { tasks.get_Item(&VARIANT::from(index)) }) else {
                continue;
            };
            if let Some(entry) = read_task(&task) {
                found.push(entry);
            }
        }
    }
    if depth >= MAX_FOLDER_DEPTH {
        return;
    }
    // SAFETY: read-only COM calls on live interfaces.
    if let Ok(folders) = unsafe { folder.GetFolders(0) }
        && let Ok(count) =
            // SAFETY: read-only COM call on a live interface.
            unsafe { folders.Count() }
    {
        for index in 1..=count {
            // SAFETY: a 1-based index within `Count`.
            if let Ok(child) = unsafe { folders.get_Item(&VARIANT::from(index)) } {
                collect(&child, depth + 1, found);
            }
        }
    }
}

/// The task as a [`LogonTask`] when it has a logon trigger and can be read.
fn read_task(task: &IRegisteredTask) -> Option<LogonTask> {
    // SAFETY: read-only COM calls on live interfaces; every out value is a
    // valid local the call writes to.
    unsafe {
        let definition = task.Definition().ok()?;
        let triggers = definition.Triggers().ok()?;
        let mut count = 0;
        triggers.Count(&raw mut count).ok()?;
        let mut logon = false;
        let mut logon_enabled = false;
        for index in 1..=count {
            let Ok(trigger) = triggers.get_Item(index) else {
                continue;
            };
            let mut kind = TASK_TRIGGER_TYPE2::default();
            if trigger.Type(&raw mut kind).is_err() || kind != TASK_TRIGGER_LOGON {
                continue;
            }
            logon = true;
            let mut enabled = VARIANT_BOOL::default();
            if trigger.Enabled(&raw mut enabled).is_ok() && enabled.as_bool() {
                logon_enabled = true;
            }
        }
        if !logon {
            return None;
        }
        let path = task.Path().ok()?.to_string();
        let task_enabled = task.Enabled().is_ok_and(VARIANT_BOOL::as_bool);

        let (mut program, mut arguments) = (String::new(), String::new());
        if let Ok(actions) = definition.Actions() {
            let mut action_count = 0;
            if actions.Count(&raw mut action_count).is_ok() {
                for index in 1..=action_count {
                    let Ok(action) = actions.get_Item(index) else {
                        continue;
                    };
                    let mut kind = TASK_ACTION_TYPE::default();
                    if action.Type(&raw mut kind).is_err() || kind != TASK_ACTION_EXEC {
                        continue;
                    }
                    if let Ok(exec) = action.cast::<IExecAction>() {
                        let (mut p, mut a) = (BSTR::new(), BSTR::new());
                        if exec.Path(&raw mut p).is_ok() {
                            program = p.to_string();
                            if exec.Arguments(&raw mut a).is_ok() {
                                arguments = a.to_string();
                            }
                            break;
                        }
                    }
                }
            }
        }
        Some(LogonTask {
            path,
            program,
            arguments,
            enabled: task_enabled && logon_enabled,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shortcut_that_does_not_exist_is_an_error_not_a_panic() {
        let result = WinShortcuts::new().resolve(Path::new(r"C:\this\does\not\exist\a.lnk"));
        assert!(result.is_err());
    }

    #[test]
    fn a_shortcut_made_in_a_temp_folder_resolves_to_its_target_and_arguments() {
        let dir = tempfile::tempdir().expect("temp dir");
        let file = dir.path().join("sample.lnk");
        let wide = to_wide_nul(&file.to_string_lossy(), "path").expect("path");
        let target = to_wide_nul(r"C:\Windows\System32\notepad.exe", "target").expect("target");
        let args = to_wide_nul("--sample", "args").expect("args");
        {
            let _com = ComScope::enter().expect("com");
            // SAFETY: test-only: builds a shortcut object, fills it from
            // NUL-terminated buffers that outlive the calls and saves it into
            // a temp folder.
            unsafe {
                let link: IShellLinkW =
                    CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).expect("link");
                link.SetPath(PCWSTR(target.as_ptr())).expect("path");
                link.SetArguments(PCWSTR(args.as_ptr())).expect("args");
                let persist: IPersistFile = link.cast().expect("persist");
                persist
                    .Save(PCWSTR(wide.as_ptr()), true)
                    .expect("save shortcut");
            }
        }
        let resolved = WinShortcuts::new()
            .resolve(&file)
            .expect("reads")
            .expect("has a target");
        assert!(resolved.target.to_lowercase().ends_with("notepad.exe"));
        assert_eq!(resolved.arguments, "--sample");
    }

    #[test]
    fn the_logon_task_list_reads_without_failing() {
        // Read-only: lists what the current user may read.
        let tasks = WinLogonTasks::new().logon_tasks().expect("reads");
        for task in tasks {
            assert!(task.path.starts_with('\\'), "{}", task.path);
        }
    }
}
