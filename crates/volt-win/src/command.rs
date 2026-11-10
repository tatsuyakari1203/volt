//! [`Commands`] port: run an external program directly, with no shell.
//!
//! How a command runs:
//!
//! - **Resolution.** The program is resolved by [`resolve_program`]. A bare name (`powercfg`) is
//!   looked up in the Windows system directory only, never in `PATH` or the current directory, and
//!   `.exe` is added when the name has no extension. An absolute path is used as given. A relative
//!   path (anything with a separator or a drive that is not absolute) is rejected, and so are UNC
//!   paths and batch scripts (`.bat`, `.cmd`), which would need a shell.
//! - **No shell, no console.** The program is started with `CreateProcess` through
//!   [`std::process::Command`] and its argument list, so each argument arrives intact. The process
//!   gets `CREATE_NO_WINDOW`, so no console window flashes, and a null standard input.
//! - **Output.** Standard output and standard error share one pipe, so the text is in the order it
//!   was written. At most [`DEFAULT_MAX_OUTPUT`] bytes are kept; the rest is drained and dropped
//!   and a note is appended to the text. Console text is decoded without ever failing (UTF-8,
//!   UTF-16 with a byte order mark, or the OEM code page, with replacement characters as the last
//!   resort).
//! - **Timeout and process tree.** The child is created suspended, added to a job object that
//!   kills its members when the job closes, and only then resumed, so it cannot start a
//!   grandchild before it is in the job and no descendant can escape. When the timeout expires the
//!   whole job is terminated and the call fails with [`PortError::TimedOut`]. When the program
//!   exits on its own, any process it left behind is killed too, so nothing outlives a command.
//!   The timeout is the adapter's, or the command's own `timeout_secs` capped at
//!   [`MAX_TIMEOUT_SECS`].
//!
//! Redaction: only the program's file name and the exit code are logged, never arguments or
//! output.

use std::ffi::c_void;
use std::fmt::Write as _;
use std::io::Read;
use std::os::windows::io::AsRawHandle as _;
use std::os::windows::process::CommandExt as _;
use std::path::{Component, Path, PathBuf, Prefix};
use std::process::Stdio;
use std::thread;
use std::time::Duration;

use volt_core::{Command, CommandOutput, Commands, MAX_TIMEOUT_SECS, PortError};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Globalization::MultiByteToWideChar;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject,
};
use windows::Win32::System::SystemInformation::GetSystemDirectoryW;
use windows::Win32::System::Threading::{
    OpenThread, ResumeThread, THREAD_SUSPEND_RESUME, WaitForSingleObject,
};
use windows::core::PCWSTR;

use crate::error::from_io;

/// How long a command may run before it is killed.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// How many bytes of output are kept.
pub const DEFAULT_MAX_OUTPUT: usize = 64 * 1024;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const CREATE_SUSPENDED: u32 = 0x0000_0004;
/// Exit code the job gives to processes it terminates.
const KILLED_EXIT_CODE: u32 = 1;
/// How long to wait for the output pipe to close once the process tree is gone.
const READER_GRACE: Duration = Duration::from_secs(5);

/// The commands port backed by `CreateProcess`.
#[derive(Debug, Clone, Copy)]
pub struct WinCommands {
    timeout: Duration,
    max_output: usize,
}

impl Default for WinCommands {
    fn default() -> Self {
        Self::new()
    }
}

impl WinCommands {
    /// An adapter with [`DEFAULT_TIMEOUT`] and [`DEFAULT_MAX_OUTPUT`].
    #[must_use]
    pub const fn new() -> Self {
        Self {
            timeout: DEFAULT_TIMEOUT,
            max_output: DEFAULT_MAX_OUTPUT,
        }
    }

    /// An adapter with its own timeout and output cap (in bytes).
    #[must_use]
    pub const fn with_limits(timeout: Duration, max_output: usize) -> Self {
        Self {
            timeout,
            max_output,
        }
    }

    /// The time `command` may run: its own limit, capped at [`MAX_TIMEOUT_SECS`], or the
    /// adapter's.
    fn timeout_for(&self, command: &Command) -> Duration {
        command.timeout_secs.map_or(self.timeout, |secs| {
            Duration::from_secs(u64::from(secs.min(MAX_TIMEOUT_SECS)))
        })
    }

    /// Runs `command`, calling `on_spawn` with the process id right after the process started.
    fn run_inner(
        &self,
        command: &Command,
        on_spawn: impl FnOnce(u32),
    ) -> Result<CommandOutput, PortError> {
        let path = resolve_program(&command.program)?;
        if command.args.iter().any(|arg| arg.contains('\0')) {
            return Err(PortError::InvalidInput(
                "an argument contains a NUL character".into(),
            ));
        }
        let name = file_name(&path);
        let context = format!("program {name}");
        log::debug!("running program {name}");

        let job = Job::new()?;
        let (reader, writer) = std::io::pipe()
            .map_err(|e| PortError::Other(format!("could not create a pipe: {e}")))?;
        let writer_err = writer
            .try_clone()
            .map_err(|e| PortError::Other(format!("could not create a pipe: {e}")))?;

        let mut process = std::process::Command::new(&path);
        process
            .args(&command.args)
            .stdin(Stdio::null())
            .stdout(writer)
            .stderr(writer_err)
            .creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
        let spawned = process.spawn();
        // The command object owns the parent's copies of the pipe's write ends. They must be
        // closed here, or the reader would never see the end of the output.
        drop(process);
        let mut child = spawned.map_err(|e| from_io(&e, &context))?;

        // The child cannot run yet: put it in the job first, then let it start.
        let started = job
            .assign(child.as_raw_handle())
            .and_then(|()| resume_process(child.id()));
        if let Err(error) = started {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        on_spawn(child.id());

        let max = self.max_output;
        let (sender, receiver) = std::sync::mpsc::channel();
        thread::spawn(move || {
            let _ = sender.send(read_capped(reader, max));
        });

        // SAFETY: the handle belongs to `child`, which is alive for this whole call.
        let waited = unsafe {
            WaitForSingleObject(
                HANDLE(child.as_raw_handle()),
                u32::try_from(self.timeout_for(command).as_millis()).unwrap_or(u32::MAX - 1),
            )
        };
        let timed_out = waited != WAIT_OBJECT_0;
        if timed_out {
            job.terminate();
        }
        let status = child
            .wait()
            .map_err(|e| PortError::Other(format!("could not wait for {name}: {e}")))?;
        // Kill anything the program left running, which also closes the pipe's write ends.
        drop(job);

        let (bytes, truncated) = receiver
            .recv_timeout(READER_GRACE)
            .map_err(|_| PortError::Other(format!("could not read the output of {name}")))?
            .map_err(|e| PortError::Other(format!("could not read the output of {name}: {e}")))?;

        if timed_out {
            log::debug!("program {name} timed out");
            return Err(PortError::TimedOut(format!(
                "{name} did not finish within {} s and was stopped",
                self.timeout_for(command).as_secs_f32()
            )));
        }
        let exit_code = status.code().unwrap_or(-1);
        log::debug!("program {name} exited with code {exit_code}");
        let mut output = decode_console(&bytes, truncated);
        if truncated {
            let _ = write!(output, "\n[output truncated at {max} bytes]");
        }
        Ok(CommandOutput { exit_code, output })
    }
}

impl Commands for WinCommands {
    fn run(&self, command: &Command) -> Result<CommandOutput, PortError> {
        self.run_inner(command, |_| {})
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(|| "?".to_owned(), |n| n.to_string_lossy().into_owned())
}

/// Resolves `program` to the file that will be started (see the module documentation for the
/// rule). Does not start anything.
///
/// # Errors
///
/// [`PortError::InvalidInput`] for an empty name, a relative path, a UNC path or a batch script;
/// [`PortError::NotFound`] when a bare name is not in the system directory.
pub(crate) fn resolve_program(program: &str) -> Result<PathBuf, PortError> {
    if program.is_empty() || program.contains('\0') {
        return Err(PortError::InvalidInput(
            "empty or malformed program name".into(),
        ));
    }
    let path = Path::new(program);
    if path.is_absolute() {
        if let Some(Component::Prefix(prefix)) = path.components().next()
            && matches!(prefix.kind(), Prefix::UNC(..) | Prefix::VerbatimUNC(..))
        {
            return Err(PortError::InvalidInput(
                "programs on network shares are not run".into(),
            ));
        }
        reject_script(path)?;
        return Ok(path.to_path_buf());
    }
    if program.contains(['\\', '/', ':']) {
        return Err(PortError::InvalidInput(format!(
            "`{program}` is a relative path; use a bare program name or an absolute path"
        )));
    }
    let mut file = PathBuf::from(program);
    match file.extension() {
        None => {
            file.set_extension("exe");
        }
        Some(ext) if ext.eq_ignore_ascii_case("exe") => {}
        Some(_) => {
            return Err(PortError::InvalidInput(format!(
                "`{program}`: only .exe programs are looked up by name"
            )));
        }
    }
    let full = system_directory()?.join(file);
    if full.is_file() {
        Ok(full)
    } else {
        Err(PortError::NotFound(format!("program {program}")))
    }
}

fn reject_script(path: &Path) -> Result<(), PortError> {
    let is_script = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("bat") || e.eq_ignore_ascii_case("cmd"));
    if is_script {
        Err(PortError::InvalidInput(
            "batch scripts need a shell and are not run".into(),
        ))
    } else {
        Ok(())
    }
}

fn system_directory() -> Result<PathBuf, PortError> {
    let mut buffer = vec![0_u16; 260];
    loop {
        // SAFETY: `buffer` is a valid writable slice for the duration of the call.
        let len = unsafe { GetSystemDirectoryW(Some(&mut buffer)) } as usize;
        if len == 0 {
            return Err(PortError::Other(
                "could not find the Windows system directory".into(),
            ));
        }
        if len < buffer.len() {
            return Ok(PathBuf::from(String::from_utf16_lossy(&buffer[..len])));
        }
        buffer.resize(len + 1, 0);
    }
}

/// Resumes the one thread of a process that was created suspended.
fn resume_process(pid: u32) -> Result<(), PortError> {
    let fail =
        |e: windows::core::Error| PortError::Other(format!("could not start the process: {e}"));
    // SAFETY: a plain snapshot of the system's threads; the handle is closed below.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) }.map_err(fail)?;
    let mut entry = THREADENTRY32 {
        dwSize: u32::try_from(size_of::<THREADENTRY32>()).unwrap_or(0),
        ..THREADENTRY32::default()
    };
    let mut resumed = false;
    // SAFETY: `snapshot` is live and `entry.dwSize` is set, as the API requires.
    let mut more = unsafe { Thread32First(snapshot, &raw mut entry) }.is_ok();
    while more {
        if entry.th32OwnerProcessID == pid {
            // SAFETY: opens a thread by id; the handle is closed right after use.
            let opened = unsafe { OpenThread(THREAD_SUSPEND_RESUME, false, entry.th32ThreadID) };
            if let Ok(thread) = opened {
                // SAFETY: `thread` is a live handle with the resume right.
                resumed |= unsafe { ResumeThread(thread) } != u32::MAX;
                // SAFETY: `thread` came from OpenThread and is closed once.
                unsafe {
                    let _ = CloseHandle(thread);
                }
            }
        }
        // SAFETY: as for `Thread32First`.
        more = unsafe { Thread32Next(snapshot, &raw mut entry) }.is_ok();
    }
    // SAFETY: `snapshot` came from CreateToolhelp32Snapshot and is closed once.
    unsafe {
        let _ = CloseHandle(snapshot);
    }
    if resumed {
        Ok(())
    } else {
        Err(PortError::Other(
            "could not start the process: its thread was not found".into(),
        ))
    }
}

/// A job object that kills its processes when it is closed.
struct Job(HANDLE);

impl Job {
    fn new() -> Result<Self, PortError> {
        // SAFETY: null attributes and name create an unnamed job with default security.
        let handle = unsafe { CreateJobObjectW(None, PCWSTR::null()) }
            .map_err(|e| PortError::Other(format!("could not create a job object: {e}")))?;
        let job = Self(handle);
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: `info` is a valid JOBOBJECT_EXTENDED_LIMIT_INFORMATION and the size passed is
        // its size; the handle is a live job object owned by `job`.
        unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                std::ptr::from_ref(&info).cast::<c_void>(),
                u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>()).unwrap_or(0),
            )
        }
        .map_err(|e| PortError::Other(format!("could not configure a job object: {e}")))?;
        Ok(job)
    }

    fn assign(&self, process: *mut c_void) -> Result<(), PortError> {
        // SAFETY: both handles are live: the job is owned by `self` and the process handle comes
        // from a child that the caller still holds.
        unsafe { AssignProcessToJobObject(self.0, HANDLE(process)) }
            .map_err(|e| PortError::Other(format!("could not add the process to a job: {e}")))
    }

    fn terminate(&self) {
        // SAFETY: the job handle is live. A failure leaves nothing more to do here.
        unsafe {
            let _ = TerminateJobObject(self.0, KILLED_EXIT_CODE);
        }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        // SAFETY: the handle came from CreateJobObjectW, is owned by this value and is closed
        // exactly once, here. Closing it kills any process still in the job.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// Reads `reader` to the end, keeping at most `max` bytes. Returns the kept bytes and whether
/// anything was dropped.
fn read_capped(mut reader: impl Read, max: usize) -> std::io::Result<(Vec<u8>, bool)> {
    let mut kept = Vec::new();
    let mut truncated = false;
    let mut chunk = [0_u8; 8192];
    loop {
        let read = match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => read,
            // A broken pipe is how the end of the output looks when the writers are killed.
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => break,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        let room = max.saturating_sub(kept.len());
        if read > room {
            truncated = true;
        }
        kept.extend_from_slice(&chunk[..read.min(room)]);
    }
    Ok((kept, truncated))
}

/// Turns console bytes into text without failing. `truncated` says the bytes were cut at an
/// arbitrary place, so an incomplete character at the end is not an encoding error.
fn decode_console(bytes: &[u8], truncated: bool) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        let units: Vec<u16> = rest
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_owned(),
        Err(e) if truncated && e.error_len().is_none() => {
            String::from_utf8_lossy(&bytes[..e.valid_up_to()]).into_owned()
        }
        Err(_) => decode_oem(bytes),
    }
}

/// Decodes `bytes` with the OEM code page, which is what console programs write in.
fn decode_oem(bytes: &[u8]) -> String {
    /// `CP_OEMCP`: the system OEM code page.
    const CP_OEMCP: u32 = 1;
    let flags = windows::Win32::Globalization::MULTI_BYTE_TO_WIDE_CHAR_FLAGS(0);
    // SAFETY: a call with no output buffer only returns the required length.
    let needed = unsafe { MultiByteToWideChar(CP_OEMCP, flags, bytes, None) };
    let Ok(len) = usize::try_from(needed) else {
        return String::from_utf8_lossy(bytes).into_owned();
    };
    if len == 0 {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    let mut wide = vec![0_u16; len];
    // SAFETY: `wide` has exactly the length the first call asked for.
    let written = unsafe { MultiByteToWideChar(CP_OEMCP, flags, bytes, Some(&mut wide)) };
    let written = usize::try_from(written).unwrap_or(0).min(len);
    String::from_utf16_lossy(&wide[..written])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE};

    // The helper is this test executable itself: the tests below start it with the
    // `helper_*` tests selected by name. Anything after the flags is an extra libtest filter,
    // which only selects nothing, so the helpers can read their own arguments from the command
    // line. Nothing here changes the machine.

    const MARK: &str = "@@";

    fn helper(name: &str, extra: &[&str]) -> Command {
        let exe = std::env::current_exe().expect("test exe");
        let mut args = vec![
            "--ignored".to_owned(),
            "--exact".to_owned(),
            "--nocapture".to_owned(),
            format!("command::tests::{name}"),
        ];
        args.extend(extra.iter().map(|s| (*s).to_owned()));
        Command::new(exe.to_string_lossy(), args)
    }

    /// The arguments after the fixed helper flags.
    fn helper_args() -> Vec<String> {
        std::env::args().skip(5).collect()
    }

    #[test]
    #[ignore = "helper process, started by the tests below"]
    fn helper_exit() {
        let code: i32 = helper_args()[0].parse().expect("code");
        println!("{MARK}hello{MARK}");
        eprintln!("{MARK}world{MARK}");
        std::process::exit(code);
    }

    #[test]
    #[ignore = "helper process, started by the tests below"]
    fn helper_args_echo() {
        for arg in helper_args() {
            println!("{MARK}{arg}{MARK}");
        }
    }

    #[test]
    #[ignore = "helper process, started by the tests below"]
    fn helper_flood() {
        let n: usize = helper_args()[0].parse().expect("n");
        println!("{MARK}{}", "x".repeat(n));
    }

    #[test]
    #[ignore = "helper process, started by the tests below"]
    fn helper_sleep() {
        thread::sleep(Duration::from_secs(120));
    }

    #[test]
    #[ignore = "helper process, started by the tests below"]
    // The grandchild is left running on purpose: the test is about the job killing it.
    #[allow(clippy::zombie_processes)]
    fn helper_orphan() {
        // Starts a grandchild at once and exits shortly after, leaving it behind.
        let file = helper_args()[0].clone();
        let exe = std::env::current_exe().expect("exe");
        let grandchild = std::process::Command::new(exe)
            .args(["--ignored", "--exact", "command::tests::helper_sleep"])
            .stdout(Stdio::null())
            .spawn()
            .expect("spawn grandchild");
        std::fs::write(file, grandchild.id().to_string()).expect("write pid");
        // Long enough for the test to open the grandchild, short next to its 120 s sleep.
        thread::sleep(Duration::from_millis(1500));
    }

    #[test]
    #[ignore = "helper process, started by the tests below"]
    fn helper_tree() {
        let file = helper_args()[0].clone();
        let exe = std::env::current_exe().expect("exe");
        let mut grandchild = std::process::Command::new(exe)
            .args(["--ignored", "--exact", "command::tests::helper_sleep"])
            .stdout(Stdio::inherit())
            .spawn()
            .expect("spawn grandchild");
        std::fs::write(file, grandchild.id().to_string()).expect("write pid");
        let _ = grandchild.wait();
    }

    fn marked(output: &str) -> Vec<String> {
        output
            .lines()
            .filter_map(|l| l.strip_prefix(MARK))
            .filter_map(|l| l.strip_suffix(MARK).map(str::to_owned))
            .collect()
    }

    fn adapter() -> WinCommands {
        WinCommands::with_limits(Duration::from_secs(60), DEFAULT_MAX_OUTPUT)
    }

    #[test]
    fn exit_codes_zero_one_and_other_are_reported() {
        for code in [0, 1, 7, 255] {
            let out = adapter()
                .run(&helper("helper_exit", &[&code.to_string()]))
                .expect("runs");
            assert_eq!(out.exit_code, code);
        }
    }

    #[test]
    fn output_combines_stdout_and_stderr() {
        let out = adapter().run(&helper("helper_exit", &["0"])).expect("runs");
        assert_eq!(marked(&out.output), ["hello", "world"]);
    }

    #[test]
    fn arguments_with_spaces_and_quotes_arrive_intact() {
        let args = [
            "plain",
            "two words",
            "say \"hi\"",
            "trailing backslash\\",
            "",
            "a&b|c>d %PATH% $x",
            "ünïcode ✓",
        ];
        let out = adapter()
            .run(&helper("helper_args_echo", &args))
            .expect("runs");
        assert_eq!(out.exit_code, 0, "{}", out.output);
        assert_eq!(marked(&out.output), args);
    }

    #[test]
    fn long_output_is_cut_and_says_so() {
        let small = WinCommands::with_limits(Duration::from_secs(60), 1000);
        let out = small
            .run(&helper("helper_flood", &["200000"]))
            .expect("runs");
        assert_eq!(out.exit_code, 0);
        assert!(out.output.contains("[output truncated at 1000 bytes]"));
        assert!(out.output.len() < 1200, "kept {} bytes", out.output.len());
    }

    #[test]
    fn short_output_is_not_marked_truncated() {
        let out = adapter().run(&helper("helper_exit", &["0"])).expect("runs");
        assert!(!out.output.contains("truncated"));
    }

    #[test]
    fn decoding_never_panics_and_handles_the_common_encodings() {
        assert_eq!(decode_console(b"plain", false), "plain");
        assert_eq!(decode_console("h\u{e9}".as_bytes(), false), "h\u{e9}");
        // Cut in the middle of a two byte character: the valid prefix stays.
        assert_eq!(decode_console(&"h\u{e9}".as_bytes()[..2], true), "h");
        // UTF-16 with a byte order mark.
        assert_eq!(decode_console(&[0xFF, 0xFE, b'o', 0, b'k', 0], false), "ok");
        // Invalid UTF-8 goes through the OEM code page and still gives text.
        assert!(decode_console(&[b'a', 0xFF, 0xFE, 0x80, b'b'], false).contains('a'));
        assert_eq!(decode_console(&[], false), "");
    }

    #[test]
    fn a_program_that_does_not_exist_is_not_found() {
        let err = adapter()
            .run(&Command::new("volt-no-such-program", ["x"]))
            .expect_err("missing");
        assert!(matches!(err, PortError::NotFound(_)), "{err:?}");
        let err = adapter()
            .run(&Command::new(
                r"C:\volt-no-such-dir\nothing.exe",
                Vec::<String>::new(),
            ))
            .expect_err("missing");
        assert!(matches!(err, PortError::NotFound(_)), "{err:?}");
    }

    #[test]
    fn bare_names_resolve_to_the_system_directory_only() {
        let system = system_directory().expect("system dir");
        let cmd = resolve_program("cmd").expect("cmd");
        assert_eq!(cmd, system.join("cmd.exe"));
        assert_eq!(
            resolve_program("CMD.EXE").expect("cmd"),
            system.join("CMD.EXE")
        );
    }

    #[test]
    fn absolute_paths_are_used_as_given() {
        let exe = std::env::current_exe().expect("exe");
        assert_eq!(resolve_program(&exe.to_string_lossy()).expect("abs"), exe);
    }

    #[test]
    fn relative_paths_unc_paths_scripts_and_odd_names_are_rejected() {
        for bad in [
            "",
            r".\tool.exe",
            "./tool.exe",
            r"sub\tool.exe",
            "..\\tool.exe",
            "C:tool.exe",
            r"\\server\share\tool.exe",
            "tool.bat",
            r"C:\x\tool.cmd",
            "tool.com",
            "nul\0.exe",
        ] {
            let err = resolve_program(bad).expect_err(bad);
            assert!(matches!(err, PortError::InvalidInput(_)), "{bad}: {err:?}");
        }
    }

    #[test]
    fn nul_in_an_argument_is_rejected() {
        let err = adapter()
            .run(&Command::new("cmd", ["a\0b"]))
            .expect_err("nul");
        assert!(matches!(err, PortError::InvalidInput(_)), "{err:?}");
    }

    fn open_for_wait(pid: u32) -> Option<usize> {
        // SAFETY: plain query; the returned handle is only used by `is_gone` and never closed
        // (the test process is short lived).
        unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) }
            .ok()
            .map(|h| h.0 as usize)
    }

    fn is_gone(handle: usize) -> bool {
        // SAFETY: the handle was opened by `open_for_wait` and is still open.
        unsafe { WaitForSingleObject(HANDLE(handle as *mut c_void), 5000) == WAIT_OBJECT_0 }
    }

    #[test]
    fn timeout_kills_the_child_and_reports_a_typed_error() {
        let short = WinCommands::with_limits(Duration::from_millis(1500), DEFAULT_MAX_OUTPUT);
        let (tx, rx) = mpsc::channel();
        let started = std::time::Instant::now();
        let result = short.run_inner(&helper("helper_sleep", &[]), |pid| {
            tx.send(open_for_wait(pid)).expect("send");
        });
        assert!(started.elapsed() < Duration::from_secs(30));
        let Err(PortError::TimedOut(text)) = &result else {
            panic!("expected a timeout, got {result:?}");
        };
        assert!(text.contains("did not finish"), "{text}");
        let handle = rx.recv().expect("pid").expect("open process");
        assert!(is_gone(handle), "the child is still running");
    }

    #[test]
    fn a_command_states_its_own_timeout_and_the_bound_caps_it() {
        let adapter = adapter();
        let plain = Command::new("cmd", ["/c", "exit"]);
        assert_eq!(adapter.timeout_for(&plain), Duration::from_secs(60));
        assert_eq!(
            adapter.timeout_for(&plain.clone().with_timeout_secs(5)),
            Duration::from_secs(5)
        );
        assert_eq!(
            adapter.timeout_for(&plain.with_timeout_secs(u32::MAX)),
            Duration::from_secs(u64::from(MAX_TIMEOUT_SECS))
        );

        // The command's own limit stops it, not the adapter's 60 s.
        let started = std::time::Instant::now();
        let result = adapter.run(&helper("helper_sleep", &[]).with_timeout_secs(1));
        assert!(matches!(result, Err(PortError::TimedOut(_))), "{result:?}");
        assert!(started.elapsed() < Duration::from_secs(30));
    }

    #[test]
    fn harmless_system_programs_run_and_a_slow_one_times_out() {
        let out = adapter()
            .run(&Command::new("cmd", ["/c", "exit", "3"]))
            .expect("cmd runs");
        assert_eq!(out.exit_code, 3);
        let out = adapter()
            .run(&Command::new("ping", ["-n", "1", "127.0.0.1"]))
            .expect("ping runs");
        assert_eq!(out.exit_code, 0, "{}", out.output);
        // `ping -n 30` to loopback takes about half a minute; the limit stops it after one
        // second.
        let slow = Command::new("ping", ["-n", "30", "127.0.0.1"]).with_timeout_secs(1);
        let result = adapter().run(&slow);
        assert!(matches!(result, Err(PortError::TimedOut(_))), "{result:?}");
    }

    /// Runs `helper_name` with a pid file, polls for the grandchild it records and returns the
    /// result plus a handle to the grandchild.
    fn run_with_grandchild(
        helper_name: &str,
        timeout: Duration,
    ) -> (Result<CommandOutput, PortError>, usize) {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("pid.txt");
        let short = WinCommands::with_limits(timeout, DEFAULT_MAX_OUTPUT);

        let pid_path = file.clone();
        let poller = thread::spawn(move || {
            for _ in 0..200 {
                if let Ok(text) = std::fs::read_to_string(&pid_path)
                    && let Ok(pid) = text.trim().parse::<u32>()
                    && let Some(handle) = open_for_wait(pid)
                {
                    return Some(handle);
                }
                thread::sleep(Duration::from_millis(50));
            }
            None
        });

        let started = std::time::Instant::now();
        let result = short.run_inner(&helper(helper_name, &[&file.to_string_lossy()]), |_| {});
        // The grandchild sleeps for 120 s: finishing early means the tree was killed, not waited
        // out.
        assert!(started.elapsed() < Duration::from_secs(30));
        let grandchild = poller.join().expect("poller").expect("grandchild started");
        (result, grandchild)
    }

    #[test]
    fn timeout_kills_grandchildren_too() {
        let (result, grandchild) = run_with_grandchild("helper_tree", Duration::from_secs(6));
        assert!(matches!(result, Err(PortError::TimedOut(_))), "{result:?}");
        assert!(is_gone(grandchild), "the grandchild is still running");
    }

    #[test]
    fn a_grandchild_started_at_once_never_escapes_the_job() {
        // The child starts a grandchild as its very first act and exits. Created suspended and
        // resumed only after it joined the job, the grandchild is always in the job too and dies
        // with it. Repeating the run makes a race show up.
        for _ in 0..5 {
            let (result, grandchild) =
                run_with_grandchild("helper_orphan", Duration::from_secs(60));
            assert!(result.is_ok(), "{result:?}");
            assert!(is_gone(grandchild), "a grandchild escaped the job");
        }
    }

    #[test]
    fn a_real_read_only_system_program_runs() {
        // `hostname` only prints the machine name.
        let out = adapter()
            .run(&Command::new("hostname", Vec::<String>::new()))
            .expect("hostname runs");
        assert_eq!(out.exit_code, 0);
        assert!(!out.output.trim().is_empty());
    }
}
