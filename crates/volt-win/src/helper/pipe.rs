//! The helper's named pipe, server and client side.
//!
//! The server creates the pipe with the first-instance flag and a maximum of one instance, so no
//! other process can serve the same name, and with remote clients rejected. It is a MESSAGE pipe:
//! one write is one message, so a request is never split or glued together. The security
//! descriptor ([`pipe_sddl`]) lets only SYSTEM, Administrators and the helper's own user open it,
//! gives the user no right to create another instance, and refuses Low integrity and `AppContainer`.
//!
//! All I/O is overlapped with a timeout, so a peer that goes quiet can never hold the caller for
//! longer than the caller allows, and the helper's idle and hard limits need no extra thread.
//!
//! The client half carries the checks the UI makes before it sends anything: who owns the pipe
//! ([`PipeClient::owner_sid`], [`is_trusted_owner`]) and whether the server is the process the UI
//! started ([`PipeClient::server_is`]).

use std::time::Duration;

use windows::Win32::Foundation::{
    ERROR_BROKEN_PIPE, ERROR_FILE_NOT_FOUND, ERROR_IO_PENDING, ERROR_MORE_DATA, ERROR_NO_DATA,
    ERROR_OPERATION_ABORTED, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, ERROR_PIPE_NOT_CONNECTED,
    GENERIC_READ, HLOCAL, INVALID_HANDLE_VALUE, LocalFree, WAIT_OBJECT_0,
};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, FILE_FLAGS_AND_ATTRIBUTES,
    FILE_SHARE_NONE, FILE_WRITE_ATTRIBUTES, FILE_WRITE_DATA, OPEN_EXISTING, PIPE_ACCESS_DUPLEX,
    ReadFile, SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT, WriteFile,
};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeClientProcessId,
    GetNamedPipeServerProcessId, NAMED_PIPE_MODE, PIPE_READMODE_MESSAGE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_MESSAGE, PIPE_WAIT, SetNamedPipeHandleState,
};
use windows::Win32::System::Threading::{CreateEventW, INFINITE, ResetEvent, WaitForSingleObject};
use windows::core::{Error, PCWSTR};

use super::acl::{self, SecurityInfo};
use super::args::PipeName;
use super::identity::ProcessHandle;
use super::{OwnedHandle, failed, wide};

/// The access the user is granted on the pipe: read data, write data and attributes, extended
/// attributes, read control and synchronise. It is generic read and write WITHOUT
/// `FILE_APPEND_DATA`, which is `FILE_CREATE_PIPE_INSTANCE` on a pipe, so the user's own processes
/// cannot create a second instance of the pipe and pose as the server.
pub const USER_PIPE_MASK: u32 = 0x0012_019b;

/// `FILE_CREATE_PIPE_INSTANCE`, the same bit as `FILE_APPEND_DATA`.
pub const FILE_CREATE_PIPE_INSTANCE: u32 = 0x4;

/// Size of the pipe's in and out buffers.
const BUFFER_BYTES: u32 = 4096;

/// SID of SYSTEM.
const SID_SYSTEM: &str = "S-1-5-18";
/// SID of the Administrators group.
const SID_ADMINISTRATORS: &str = "S-1-5-32-544";

/// The pipe's security descriptor in SDDL for a helper running as `user_sid`.
///
/// - `D:P`: a protected DACL, nothing inherited.
/// - SYSTEM and Administrators: full access. The Administrators entry is deny-only in the UI's
///   split token, so it grants the UI nothing.
/// - the user: [`USER_PIPE_MASK`], and nobody else is listed, so everyone else is denied.
/// - `S:(ML;;NW;;;ME)`: a Medium mandatory label that forbids write-up, so Low integrity and
///   `AppContainer` processes cannot open the pipe for writing.
#[must_use]
pub fn pipe_sddl(user_sid: &str) -> String {
    format!("D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;0x12019b;;;{user_sid})S:(ML;;NW;;;ME)")
}

/// Whether the pipe's owner is one a medium-integrity process cannot create: SYSTEM or
/// Administrators. A pipe owned by anyone else was not made by a properly elevated helper.
#[must_use]
pub fn is_trusted_owner(owner_sid: &str) -> bool {
    owner_sid == SID_SYSTEM || owner_sid == SID_ADMINISTRATORS
}

/// Why a pipe call did not give a result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IoError {
    /// Nothing happened within the time allowed. The call was cancelled.
    Timeout,
    /// The other side closed the pipe (or is not connected).
    Closed,
    /// A message was longer than the caller's limit. The rest of it is discarded by closing.
    TooLarge,
    /// Any other failure.
    Os(String),
}

impl std::fmt::Display for IoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Timeout => f.write_str("timed out"),
            Self::Closed => f.write_str("the pipe is closed"),
            Self::TooLarge => f.write_str("message too large"),
            Self::Os(message) => f.write_str(message),
        }
    }
}

/// One end of a pipe with the event its overlapped calls wait on.
#[derive(Debug)]
struct Pipe {
    handle: OwnedHandle,
    event: OwnedHandle,
}

fn new_event() -> Result<OwnedHandle, String> {
    // SAFETY: a manual-reset, initially unsignalled, unnamed event with default security.
    unsafe { CreateEventW(None, true, false, PCWSTR::null()) }
        .map(OwnedHandle)
        .map_err(|e| failed("CreateEventW", &e))
}

/// Milliseconds for `WaitForSingleObject`: `None` waits forever, a long wait is capped just
/// below `INFINITE`.
fn wait_millis(timeout: Option<Duration>) -> u32 {
    timeout.map_or(INFINITE, |limit| {
        u32::try_from(limit.as_millis()).map_or(INFINITE - 1, |ms| ms.min(INFINITE - 1))
    })
}

fn map_error(error: &Error) -> IoError {
    let code = error.code();
    if code == ERROR_BROKEN_PIPE.to_hresult()
        || code == ERROR_NO_DATA.to_hresult()
        || code == ERROR_PIPE_NOT_CONNECTED.to_hresult()
    {
        IoError::Closed
    } else if code == ERROR_MORE_DATA.to_hresult() {
        // Only a read can report this: the message did not fit the buffer.
        IoError::TooLarge
    } else {
        IoError::Os(error.to_string())
    }
}

impl Pipe {
    fn new(handle: OwnedHandle) -> Result<Self, String> {
        Ok(Self {
            handle,
            event: new_event()?,
        })
    }

    /// Runs one overlapped call and waits for it for at most `timeout`. `start` issues the call
    /// with the `OVERLAPPED` it is given and returns `Ok(true)` when the call finished at once
    /// with nothing to collect. On a timeout the call is cancelled and drained before this returns,
    /// so the `OVERLAPPED` and any buffer outlive the kernel's use of them. Returns the byte count.
    fn overlapped<F>(&self, timeout: Option<Duration>, start: F) -> Result<u32, IoError>
    where
        F: FnOnce(*mut OVERLAPPED) -> windows::core::Result<bool>,
    {
        let mut overlapped = OVERLAPPED {
            hEvent: self.event.raw(),
            ..OVERLAPPED::default()
        };
        // SAFETY: a valid event handle owned by this pipe.
        unsafe { ResetEvent(self.event.raw()) }
            .map_err(|e| IoError::Os(failed("ResetEvent", &e)))?;
        match start(&raw mut overlapped) {
            Ok(true) => return Ok(0),
            Ok(false) => {}
            Err(error) if error.code() == ERROR_IO_PENDING.to_hresult() => {}
            Err(error) => return Err(map_error(&error)),
        }
        // SAFETY: a valid event handle; the wait has no other precondition.
        let waited = unsafe { WaitForSingleObject(self.event.raw(), wait_millis(timeout)) };
        let mut transferred = 0_u32;
        if waited != WAIT_OBJECT_0 {
            // Timed out (or the wait failed): cancel, then wait for the kernel to let go of the
            // OVERLAPPED. The call may have completed in the meantime, which counts as success.
            // SAFETY: a valid handle and the OVERLAPPED the call was issued with.
            let _ = unsafe { CancelIoEx(self.handle.raw(), Some(&raw const overlapped)) };
            // SAFETY: as above; `true` waits until the cancelled call has finished.
            return match unsafe {
                GetOverlappedResult(
                    self.handle.raw(),
                    &raw const overlapped,
                    &raw mut transferred,
                    true,
                )
            } {
                Ok(()) => Ok(transferred),
                Err(error) if error.code() == ERROR_OPERATION_ABORTED.to_hresult() => {
                    Err(IoError::Timeout)
                }
                Err(error) => Err(map_error(&error)),
            };
        }
        // SAFETY: a valid handle and the OVERLAPPED the call was issued with; the event is set, so
        // the call has finished and `false` does not wait.
        unsafe {
            GetOverlappedResult(
                self.handle.raw(),
                &raw const overlapped,
                &raw mut transferred,
                false,
            )
        }
        .map_err(|error| map_error(&error))?;
        Ok(transferred)
    }

    /// Reads one message of at most `max` bytes.
    fn read_message(&self, max: usize, timeout: Option<Duration>) -> Result<Vec<u8>, IoError> {
        // One byte more than the limit, so a message that is too long is seen as such.
        let mut buffer = vec![0_u8; max.saturating_add(1)];
        let read = self.overlapped(timeout, |overlapped| {
            // SAFETY: `buffer` outlives the call (the overlapped helper waits for it to finish)
            // and `overlapped` is the OVERLAPPED it was given.
            match unsafe { ReadFile(self.handle.raw(), Some(&mut buffer), None, Some(overlapped)) }
            {
                Ok(()) => Ok(false),
                Err(error) => Err(error),
            }
        })? as usize;
        if read > max {
            return Err(IoError::TooLarge);
        }
        buffer.truncate(read);
        Ok(buffer)
    }

    /// Writes one message.
    fn write_message(&self, message: &[u8], timeout: Option<Duration>) -> Result<(), IoError> {
        let written = self.overlapped(timeout, |overlapped| {
            // SAFETY: `message` outlives the call (the overlapped helper waits for it to finish)
            // and `overlapped` is the OVERLAPPED it was given.
            match unsafe { WriteFile(self.handle.raw(), Some(message), None, Some(overlapped)) } {
                Ok(()) => Ok(false),
                Err(error) => Err(error),
            }
        })?;
        if written as usize == message.len() {
            Ok(())
        } else {
            Err(IoError::Os("short write on the pipe".to_owned()))
        }
    }

    /// Who owns the pipe, and what its DACL says.
    fn security(&self) -> Result<SecurityInfo, String> {
        acl::of_kernel_handle(&self.handle)
    }
}

// ---------------------------------------------------------------------------------------------
// Server

/// The server's pipe: one instance, for the life of the process.
#[derive(Debug)]
pub struct PipeServer {
    pipe: Pipe,
}

impl PipeServer {
    /// Creates the pipe for `name` with the descriptor [`pipe_sddl`] builds for `user_sid`.
    /// Fails when the name is already in use (first-instance flag), so no one else can serve it.
    ///
    /// # Errors
    /// A message when the descriptor or the pipe cannot be created.
    pub fn create(name: &PipeName, user_sid: &str) -> Result<Self, String> {
        Self::create_with_sddl(name, &pipe_sddl(user_sid))
    }

    fn create_with_sddl(name: &PipeName, sddl: &str) -> Result<Self, String> {
        let sddl = wide(sddl);
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        // SAFETY: `sddl` is NUL-terminated and outlives the call; `descriptor` is a valid out
        // pointer. The allocated descriptor is freed below once the pipe has copied it.
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(sddl.as_ptr()),
                SDDL_REVISION_1,
                &raw mut descriptor,
                None,
            )
        }
        .map_err(|e| failed("ConvertStringSecurityDescriptorToSecurityDescriptorW", &e))?;
        let attributes = SECURITY_ATTRIBUTES {
            nLength: u32::try_from(std::mem::size_of::<SECURITY_ATTRIBUTES>()).unwrap_or(0),
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: false.into(),
        };
        let path = wide(&name.full_path());
        let mode: NAMED_PIPE_MODE =
            PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS;
        // SAFETY: `path` is NUL-terminated; `attributes` points at a valid descriptor and both
        // outlive the call (the kernel copies the descriptor). One instance at most.
        let handle = unsafe {
            CreateNamedPipeW(
                PCWSTR(path.as_ptr()),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE | FILE_FLAG_OVERLAPPED,
                mode,
                1,
                BUFFER_BYTES,
                BUFFER_BYTES,
                0,
                Some(&raw const attributes),
            )
        };
        let created = if handle == INVALID_HANDLE_VALUE {
            Err(failed("CreateNamedPipeW", &Error::from_thread()))
        } else {
            Ok(OwnedHandle(handle))
        };
        // SAFETY: `descriptor.0` was allocated by the conversion call above and is freed once.
        let _ = unsafe { LocalFree(Some(HLOCAL(descriptor.0))) };
        Ok(Self {
            pipe: Pipe::new(created?)?,
        })
    }

    /// Waits for a client to connect, at most `timeout`. A client that connected before this call
    /// counts as connected.
    ///
    /// # Errors
    /// [`IoError::Timeout`] when nobody connected in time.
    pub fn accept(&self, timeout: Option<Duration>) -> Result<(), IoError> {
        self.pipe.overlapped(timeout, |overlapped| {
            // SAFETY: a valid pipe handle and the OVERLAPPED the helper passes.
            match unsafe { ConnectNamedPipe(self.pipe.handle.raw(), Some(overlapped)) } {
                Ok(()) => Ok(false),
                // The client was already there: nothing was queued and there is nothing to wait for.
                Err(error) if error.code() == ERROR_PIPE_CONNECTED.to_hresult() => Ok(true),
                Err(error) => Err(error),
            }
        })?;
        Ok(())
    }

    /// The process id of the connected client, as the system reports it.
    ///
    /// # Errors
    /// A message when no client is connected.
    pub fn client_pid(&self) -> Result<u32, String> {
        let mut pid = 0_u32;
        // SAFETY: a valid pipe handle and a valid out pointer.
        unsafe { GetNamedPipeClientProcessId(self.pipe.handle.raw(), &raw mut pid) }
            .map_err(|e| failed("GetNamedPipeClientProcessId", &e))?;
        Ok(pid)
    }

    /// Reads one message of at most `max` bytes, waiting at most `timeout`.
    ///
    /// # Errors
    /// [`IoError`].
    pub fn read_message(&self, max: usize, timeout: Option<Duration>) -> Result<Vec<u8>, IoError> {
        self.pipe.read_message(max, timeout)
    }

    /// Writes one message, waiting at most `timeout`.
    ///
    /// # Errors
    /// [`IoError`].
    pub fn write_message(&self, message: &[u8], timeout: Option<Duration>) -> Result<(), IoError> {
        self.pipe.write_message(message, timeout)
    }

    /// Ends the current client's connection so the next [`accept`](Self::accept) can run. Data the
    /// client has not read is discarded, so a caller that wants the client to see its last reply
    /// uses [`finish`](Self::finish).
    pub fn disconnect(&self) {
        // SAFETY: a valid pipe handle. A failure means the client is already gone.
        let _ = unsafe { DisconnectNamedPipe(self.pipe.handle.raw()) };
    }

    /// Lets the client read what was written, then disconnects: waits up to `grace` for the client
    /// to close its end (it does so after reading its reply), discarding anything it still sends.
    pub fn finish(&self, grace: Duration) {
        let deadline = std::time::Instant::now() + grace;
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                break;
            }
            match self.pipe.read_message(DRAIN_BYTES, Some(left)) {
                Ok(_) | Err(IoError::TooLarge) => {}
                Err(_) => break,
            }
        }
        self.disconnect();
    }

    /// Owner and DACL of the pipe, read back from the system.
    ///
    /// # Errors
    /// A message when they cannot be read.
    pub fn security(&self) -> Result<SecurityInfo, String> {
        self.pipe.security()
    }
}

/// How much a client may still send while the server waits for it to close.
const DRAIN_BYTES: usize = 1024;

// ---------------------------------------------------------------------------------------------
// Client

/// Why opening the pipe did not work (yet).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenError {
    /// The pipe does not exist (yet): the server is not up.
    NotThere,
    /// Another client holds the single instance.
    Busy,
    /// Any other failure (access denied, for example).
    Other(String),
}

/// A client connection to the helper's pipe.
#[derive(Debug)]
pub struct PipeClient {
    pipe: Pipe,
}

impl PipeClient {
    /// Opens the pipe once, in message mode. The client asks for exactly what the descriptor
    /// grants the user and for identification-level impersonation only, so the server can see who
    /// connected but cannot act as the client.
    ///
    /// # Errors
    /// [`OpenError`].
    pub fn open(name: &PipeName) -> Result<Self, OpenError> {
        let path = wide(&name.full_path());
        // SAFETY: `path` is NUL-terminated and outlives the call.
        let opened = unsafe {
            CreateFileW(
                PCWSTR(path.as_ptr()),
                GENERIC_READ.0 | FILE_WRITE_DATA.0 | FILE_WRITE_ATTRIBUTES.0,
                FILE_SHARE_NONE,
                None,
                OPEN_EXISTING,
                FILE_FLAGS_AND_ATTRIBUTES(
                    FILE_FLAG_OVERLAPPED.0 | SECURITY_SQOS_PRESENT.0 | SECURITY_IDENTIFICATION.0,
                ),
                None,
            )
        };
        let handle = match opened {
            Ok(handle) => OwnedHandle(handle),
            Err(error) if error.code() == ERROR_FILE_NOT_FOUND.to_hresult() => {
                return Err(OpenError::NotThere);
            }
            Err(error) if error.code() == ERROR_PIPE_BUSY.to_hresult() => {
                return Err(OpenError::Busy);
            }
            Err(error) => return Err(OpenError::Other(failed("CreateFileW (pipe)", &error))),
        };
        let mode = PIPE_READMODE_MESSAGE;
        // SAFETY: a valid pipe handle and a valid pointer to the mode, which outlives the call.
        unsafe { SetNamedPipeHandleState(handle.raw(), Some(&raw const mode), None, None) }
            .map_err(|e| OpenError::Other(failed("SetNamedPipeHandleState", &e)))?;
        Ok(Self {
            pipe: Pipe::new(handle).map_err(OpenError::Other)?,
        })
    }

    /// Opens the pipe, retrying while it does not exist yet or another client holds it, for at
    /// most `timeout`: the helper needs a moment after it is started, and a pipe that has just
    /// been disconnected is busy until the server listens again.
    ///
    /// # Errors
    /// The last [`OpenError`] when the time is up, or at once for any other failure.
    pub fn open_waiting(name: &PipeName, timeout: Duration) -> Result<Self, OpenError> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            match Self::open(name) {
                Err(OpenError::NotThere | OpenError::Busy)
                    if std::time::Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(15));
                }
                other => return other,
            }
        }
    }

    /// The process id of the server, as the system reports it.
    ///
    /// # Errors
    /// A message when Windows cannot tell.
    pub fn server_pid(&self) -> Result<u32, String> {
        let mut pid = 0_u32;
        // SAFETY: a valid pipe handle and a valid out pointer.
        unsafe { GetNamedPipeServerProcessId(self.pipe.handle.raw(), &raw mut pid) }
            .map_err(|e| failed("GetNamedPipeServerProcessId", &e))?;
        Ok(pid)
    }

    /// Whether the server is the process `launched` refers to. The UI keeps the process handle its
    /// launch call returned and checks it here before it sends anything. Not being able to tell is
    /// `false`.
    #[must_use]
    pub fn server_is(&self, launched: &ProcessHandle) -> bool {
        matches!((self.server_pid(), launched.pid()), (Ok(server), Ok(launched)) if server == launched)
    }

    /// The pipe's owner as a SID string. A properly elevated helper creates a pipe owned by
    /// Administrators or SYSTEM, which a medium-integrity process cannot make: see
    /// [`is_trusted_owner`].
    ///
    /// # Errors
    /// A message when the owner cannot be read; the caller must then send nothing.
    pub fn owner_sid(&self) -> Result<String, String> {
        self.pipe.security().map(|info| info.owner)
    }

    /// Reads one message of at most `max` bytes, waiting at most `timeout`.
    ///
    /// # Errors
    /// [`IoError`].
    pub fn read_message(&self, max: usize, timeout: Option<Duration>) -> Result<Vec<u8>, IoError> {
        self.pipe.read_message(max, timeout)
    }

    /// Writes one message, waiting at most `timeout`.
    ///
    /// # Errors
    /// [`IoError`].
    pub fn write_message(&self, message: &[u8], timeout: Option<Duration>) -> Result<(), IoError> {
        self.pipe.write_message(message, timeout)
    }
}

#[cfg(test)]
mod tests {
    use std::thread::{self, JoinHandle};

    use super::*;
    use crate::helper::identity::inspect_current;

    const SECOND: Option<Duration> = Some(Duration::from_secs(5));

    fn own_sid() -> String {
        inspect_current().expect("own token").token.user_sid
    }

    fn server() -> (PipeName, PipeServer) {
        let name = PipeName::random().expect("name");
        let server = PipeServer::create(&name, &own_sid()).expect("pipe");
        (name, server)
    }

    /// Runs `work` on another thread with the server, and hands the server back with the result.
    fn serving<T: Send + 'static>(
        server: PipeServer,
        work: impl FnOnce(&PipeServer) -> T + Send + 'static,
    ) -> JoinHandle<(PipeServer, T)> {
        thread::spawn(move || {
            let result = work(&server);
            (server, result)
        })
    }

    #[test]
    fn the_descriptor_text_is_the_one_in_the_decision() {
        assert_eq!(
            pipe_sddl("S-1-5-21-1-2-3-1001"),
            "D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;0x12019b;;;S-1-5-21-1-2-3-1001)S:(ML;;NW;;;ME)"
        );
        // FILE_CREATE_PIPE_INSTANCE is absent from the user's mask.
        assert_eq!(USER_PIPE_MASK & FILE_CREATE_PIPE_INSTANCE, 0);
    }

    #[test]
    fn only_system_and_administrators_are_trusted_owners() {
        assert!(is_trusted_owner("S-1-5-18"));
        assert!(is_trusted_owner("S-1-5-32-544"));
        assert!(!is_trusted_owner(&own_sid()));
        assert!(!is_trusted_owner("S-1-1-0"));
        assert!(!is_trusted_owner(""));
    }

    #[test]
    fn messages_keep_their_boundaries_in_both_directions() {
        let (name, server) = server();
        let thread = serving(server, |server| {
            server.accept(SECOND).expect("accept");
            let first = server.read_message(1024, SECOND).expect("first");
            let second = server.read_message(1024, SECOND).expect("second");
            let empty = server.read_message(1024, SECOND).expect("empty");
            server.write_message(b"one", SECOND).expect("reply one");
            server.write_message(b"two", SECOND).expect("reply two");
            (first, second, empty)
        });
        let client = PipeClient::open(&name).expect("open");
        client.write_message(b"hello", SECOND).expect("write");
        client.write_message(br#"{"id":1}"#, SECOND).expect("write");
        client.write_message(b"", SECOND).expect("write empty");
        assert_eq!(client.read_message(1024, SECOND).expect("read"), b"one");
        assert_eq!(client.read_message(1024, SECOND).expect("read"), b"two");
        let (_server, (first, second, empty)) = thread.join().expect("server thread");
        assert_eq!(first, b"hello");
        assert_eq!(second, br#"{"id":1}"#);
        assert!(empty.is_empty());
    }

    #[test]
    fn an_oversize_message_is_reported_not_truncated() {
        let (name, server) = server();
        let thread = serving(server, |server| {
            server.accept(SECOND).expect("accept");
            let at_limit = server.read_message(8, SECOND);
            let over = server.read_message(8, SECOND);
            (at_limit, over)
        });
        let client = PipeClient::open(&name).expect("open");
        client.write_message(b"12345678", SECOND).expect("write");
        client.write_message(b"123456789", SECOND).expect("write");
        let (_server, (at_limit, over)) = thread.join().expect("server thread");
        assert_eq!(at_limit, Ok(b"12345678".to_vec()));
        assert_eq!(over, Err(IoError::TooLarge));
    }

    #[test]
    fn reads_and_accepts_time_out_instead_of_hanging() {
        let (name, server) = server();
        let started = std::time::Instant::now();
        assert_eq!(
            server.accept(Some(Duration::from_millis(100))),
            Err(IoError::Timeout)
        );
        // The pipe still works after a cancelled accept.
        let thread = serving(server, |server| {
            server.accept(SECOND).expect("accept");
            server.read_message(16, Some(Duration::from_millis(150)))
        });
        let client = PipeClient::open(&name).expect("open");
        let (_server, read) = thread.join().expect("server thread");
        assert_eq!(read, Err(IoError::Timeout));
        assert!(started.elapsed() < Duration::from_secs(4));
        drop(client);
    }

    #[test]
    fn a_closed_client_is_seen_as_closed() {
        let (name, server) = server();
        let (accepted_tx, accepted_rx) = std::sync::mpsc::channel();
        let thread = serving(server, move |server| {
            server.accept(SECOND).expect("accept");
            accepted_tx.send(()).expect("signal");
            server.read_message(16, SECOND)
        });
        let client = PipeClient::open(&name).expect("open");
        accepted_rx.recv().expect("accepted");
        drop(client);
        let (_server, read) = thread.join().expect("server thread");
        assert_eq!(read, Err(IoError::Closed));
    }

    #[test]
    fn the_server_can_serve_the_next_client_after_a_disconnect() {
        let (name, mut server) = server();
        for round in 0_u8..2 {
            let thread = serving(server, |server| {
                server.accept(SECOND).expect("accept");
                let message = server.read_message(16, SECOND).expect("read");
                server.write_message(&message, SECOND).expect("echo");
                server.finish(Duration::from_secs(2));
            });
            let client = PipeClient::open_waiting(&name, Duration::from_secs(5)).expect("open");
            client.write_message(&[round], SECOND).expect("write");
            // The last reply survives the disconnect because the server waits for the close.
            assert_eq!(client.read_message(16, SECOND).expect("echo"), vec![round]);
            drop(client);
            server = thread.join().expect("server thread").0;
        }
    }

    #[test]
    fn a_second_instance_of_the_same_name_cannot_be_made() {
        let (name, _server) = server();
        // Another server with the same name: refused by the first-instance flag.
        assert!(PipeServer::create(&name, &own_sid()).is_err());
        // The user's own process cannot add an instance by other means either: the descriptor
        // leaves FILE_CREATE_PIPE_INSTANCE out of its mask, and the maximum is one instance.
        let path = wide(&name.full_path());
        // SAFETY: `path` is NUL-terminated and outlives the call. The call only tries to add an
        // instance to an existing pipe and is expected to fail.
        let second = unsafe {
            CreateNamedPipeW(
                PCWSTR(path.as_ptr()),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED,
                PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT,
                1,
                BUFFER_BYTES,
                BUFFER_BYTES,
                0,
                None,
            )
        };
        assert_eq!(second, INVALID_HANDLE_VALUE);
    }

    #[test]
    fn the_pipe_carries_the_descriptor_of_the_decision() {
        let (_name, server) = server();
        let me = own_sid();
        let info = server.security().expect("security");
        let dacl = info.dacl.expect("a DACL, not a NULL one");
        // Exactly three entries, all grants: SYSTEM, Administrators, and the user.
        assert_eq!(dacl.len(), 3, "{dacl:?}");
        assert!(dacl.iter().all(|ace| ace.allow));
        let mut sids: Vec<&str> = dacl.iter().map(|ace| ace.sid.as_str()).collect();
        sids.sort_unstable();
        let mut expected = vec![SID_SYSTEM, SID_ADMINISTRATORS, me.as_str()];
        expected.sort_unstable();
        assert_eq!(sids, expected);
        let user = dacl.iter().find(|ace| ace.sid == me).expect("user entry");
        assert_eq!(user.mask, USER_PIPE_MASK);
        assert_eq!(user.mask & FILE_CREATE_PIPE_INSTANCE, 0);
        // Nobody else: no Everyone, Users or Authenticated Users entry.
        for forbidden in ["S-1-1-0", "S-1-5-32-545", "S-1-5-11"] {
            assert!(dacl.iter().all(|ace| ace.sid != forbidden), "{forbidden}");
        }
    }

    #[test]
    fn the_server_is_this_process_and_an_unelevated_owner_is_not_trusted() {
        let (name, server) = server();
        let thread = serving(server, |server| server.accept(SECOND));
        let client = PipeClient::open(&name).expect("open");
        let (server, accepted) = thread.join().expect("server thread");
        accepted.expect("accept");

        let me = ProcessHandle::open(std::process::id()).expect("own handle");
        assert!(client.server_is(&me));
        assert_eq!(client.server_pid(), Ok(std::process::id()));
        assert_eq!(server.client_pid(), Ok(std::process::id()));

        // A handle of any other process does not match.
        let other = std::process::Command::new("cmd")
            .args(["/C", "ping -n 3 127.0.0.1 > nul"])
            .stdout(std::process::Stdio::null())
            .spawn();
        if let Ok(mut other) = other {
            let handle = ProcessHandle::open(other.id()).expect("other handle");
            assert!(!client.server_is(&handle));
            let _ = other.kill();
            let _ = other.wait();
        }

        // This unelevated test process made the pipe, so its owner is the user, not
        // Administrators or SYSTEM: the UI side would refuse to talk to it. (A real elevated
        // helper creates the pipe with an Administrators owner.)
        // An elevated runner (CI) owns it as Administrators, which is trusted, so only check
        // the unelevated case.
        let owner = client.owner_sid().expect("owner");
        if owner != "S-1-5-32-544" {
            assert!(!is_trusted_owner(&owner), "{owner}");
        }
    }

    #[test]
    fn a_missing_pipe_is_not_there() {
        let name = PipeName::random().expect("name");
        assert_eq!(PipeClient::open(&name).err(), Some(OpenError::NotThere));
    }
}
