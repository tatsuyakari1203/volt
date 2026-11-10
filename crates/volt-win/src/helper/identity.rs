//! Who is on the other end: process identity (id, creation time, image path), token facts (user,
//! session, integrity) and the decision whether the helper serves a client.
//!
//! The helper serves ONE client, the UI that started it. A client is served only when
//!
//! 1. it is the bound process: same process id, same creation time (a process id can be reused once
//!    its process has exited) and the image path of the sibling UI executable; and
//! 2. its token user is the helper's own user, in the same session, at Medium integrity or above.
//!
//! The token is read from the client PROCESS, never by impersonating the pipe client, so nothing
//! runs with the client's identity. Admission is not authentication of the UI: the same user's
//! other programs pass (2). Only (1) narrows it to the process the helper was told about, and the
//! closed request set is what limits what any admitted peer can do.

use std::ffi::c_void;

use windows::Win32::Foundation::{FILETIME, HANDLE, HLOCAL, LocalFree, WAIT_OBJECT_0};
use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows::Win32::Security::{
    GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, PSID, TOKEN_MANDATORY_LABEL,
    TOKEN_QUERY, TOKEN_USER, TokenElevation, TokenElevationType, TokenIntegrityLevel,
    TokenSessionId, TokenUser,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, GetProcessId, GetProcessTimes, OpenProcess,
    OpenProcessToken, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    QueryFullProcessImageNameW, WaitForSingleObject,
};
use windows::core::PWSTR;

use super::{OwnedHandle, failed};

/// The lowest integrity RID that is served: Medium.
pub const MEDIUM_INTEGRITY_RID: u32 = 0x2000;

/// Which process, for good: the id alone can be reused after the process exits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessIdentity {
    /// Process id.
    pub pid: u32,
    /// Creation time, in 100 ns ticks (a Windows file time).
    pub created_at_100ns: u64,
    /// Full path of the executable image.
    pub image: String,
}

/// What a token says about its owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenFacts {
    /// The user, as a SID string (`S-1-5-21-...`).
    pub user_sid: String,
    /// The Windows session the token belongs to.
    pub session_id: u32,
    /// The integrity level RID (`0x2000` Medium, `0x3000` High).
    pub integrity_rid: u32,
}

/// A process as the helper sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessFacts {
    /// Which process.
    pub identity: ProcessIdentity,
    /// Its token.
    pub token: TokenFacts,
}

/// Normalises a Windows path for comparison: drops a `\\?\` prefix, uses `\` and ignores case.
#[must_use]
pub fn normalise_path(path: &str) -> String {
    let trimmed = path.strip_prefix(r"\\?\").unwrap_or(path);
    trimmed.replace('/', "\\").to_lowercase()
}

impl ProcessIdentity {
    /// Whether `self` (a process found on the pipe) is the `bound` process.
    #[must_use]
    pub fn is_bound_process(&self, bound: &Self) -> bool {
        self.pid == bound.pid
            && self.created_at_100ns == bound.created_at_100ns
            && normalise_path(&self.image) == normalise_path(&bound.image)
    }
}

/// Why a client is not served.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientRefusal {
    /// The client could not be inspected (it exited, or Windows refused access). Not knowing is a
    /// refusal.
    Unreadable(String),
    /// Not the process the helper is bound to (other id, other creation time or other image).
    NotBound,
    /// Another user's token.
    OtherUser,
    /// Another Windows session.
    OtherSession,
    /// Below Medium integrity (Low, `AppContainer`, Untrusted).
    LowIntegrity,
}

/// Decides whether `client` is served: it must be the `bound` process and pass [`admit`].
///
/// # Errors
/// The [`ClientRefusal`] for the first rule it breaks.
pub fn check_client(
    client: &ProcessFacts,
    own: &TokenFacts,
    bound: &ProcessIdentity,
) -> Result<(), ClientRefusal> {
    if !client.identity.is_bound_process(bound) {
        return Err(ClientRefusal::NotBound);
    }
    admit(&client.token, own)
}

/// The token rules of admission: same user, same session, at least Medium integrity.
///
/// # Errors
/// [`ClientRefusal::OtherUser`], [`ClientRefusal::OtherSession`] or [`ClientRefusal::LowIntegrity`].
pub fn admit(client: &TokenFacts, own: &TokenFacts) -> Result<(), ClientRefusal> {
    if client.user_sid != own.user_sid {
        return Err(ClientRefusal::OtherUser);
    }
    if client.session_id != own.session_id {
        return Err(ClientRefusal::OtherSession);
    }
    if client.integrity_rid < MEDIUM_INTEGRITY_RID {
        return Err(ClientRefusal::LowIntegrity);
    }
    Ok(())
}

/// What the process's own token says about elevation, which decides what asking for it would do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    /// Already elevated: an administrator with UAC off, the built-in Administrator, or a process
    /// started as administrator. Elevating again shows no prompt.
    Elevated,
    /// The filtered token of an administrator: elevating shows a consent prompt (no credentials).
    Limited,
    /// Neither: a standard user, for whom elevating asks for an administrator's credentials.
    Standard,
}

/// `TokenElevationTypeLimited`.
const ELEVATION_TYPE_LIMITED: u32 = 3;

/// Classifies a token from `TokenIsElevated` and `TokenElevationType`.
#[must_use]
pub fn classify_token(is_elevated: bool, elevation_type: u32) -> TokenKind {
    if is_elevated {
        TokenKind::Elevated
    } else if elevation_type == ELEVATION_TYPE_LIMITED {
        TokenKind::Limited
    } else {
        TokenKind::Standard
    }
}

/// The first `u32` a token information class wrote.
fn first_u32(buffer: &[u64]) -> u32 {
    buffer.first().map_or(0, |word| {
        // The class writes one u32: the low half of the first word on a little-endian machine.
        u32::try_from(word & u64::from(u32::MAX)).unwrap_or(0)
    })
}

/// The elevation facts of this process's own token.
///
/// # Errors
/// A message when the token cannot be read.
pub fn current_token_kind() -> Result<TokenKind, String> {
    let mut token = HANDLE::default();
    // SAFETY: the pseudo handle of the current process is always valid; the out pointer is valid
    // and the token is closed on drop below.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token) }
        .map_err(|e| failed("OpenProcessToken", &e))?;
    let token = OwnedHandle(token);
    // TOKEN_ELEVATION is one u32 (TokenIsElevated); TOKEN_ELEVATION_TYPE is one enum of u32 size.
    let elevated = first_u32(&token_info(token.raw(), TokenElevation)?) != 0;
    let elevation_type = first_u32(&token_info(token.raw(), TokenElevationType)?);
    Ok(classify_token(elevated, elevation_type))
}

/// Reads one token information class into an 8-byte aligned buffer.
fn token_info(
    token: HANDLE,
    class: windows::Win32::Security::TOKEN_INFORMATION_CLASS,
) -> Result<Vec<u64>, String> {
    let mut needed = 0_u32;
    // SAFETY: a size query: a null buffer of length 0 and a valid out pointer for the size. The
    // call fails with ERROR_INSUFFICIENT_BUFFER by design.
    let _ = unsafe { GetTokenInformation(token, class, None, 0, &raw mut needed) };
    if needed == 0 {
        return Err(failed(
            "GetTokenInformation (size)",
            &windows::core::Error::from_thread(),
        ));
    }
    let mut buffer = vec![0_u64; (needed as usize).div_ceil(8)];
    // SAFETY: `buffer` is at least `needed` bytes, 8-byte aligned, and lives across the call.
    unsafe {
        GetTokenInformation(
            token,
            class,
            Some(buffer.as_mut_ptr().cast::<c_void>()),
            needed,
            &raw mut needed,
        )
    }
    .map_err(|e| failed("GetTokenInformation", &e))?;
    Ok(buffer)
}

/// A SID as its string form (`S-1-5-...`).
pub(crate) fn sid_to_string(sid: PSID) -> Result<String, String> {
    let mut text = PWSTR::null();
    // SAFETY: `sid` points to a valid SID that outlives the call; the call allocates a string
    // that is freed below with LocalFree, as the API requires.
    unsafe { ConvertSidToStringSidW(sid, &raw mut text) }
        .map_err(|e| failed("ConvertSidToStringSidW", &e))?;
    // SAFETY: on success `text` is a valid, NUL-terminated wide string until it is freed.
    let out = unsafe { text.to_string() }.map_err(|e| e.to_string());
    // SAFETY: `text` was allocated by the call above and is freed exactly once.
    let _ = unsafe { LocalFree(Some(HLOCAL(text.0.cast()))) };
    out
}

/// User, session and integrity of a token opened with `TOKEN_QUERY`.
fn token_facts(token: HANDLE) -> Result<TokenFacts, String> {
    let user_buffer = token_info(token, TokenUser)?;
    // SAFETY: the buffer holds a TOKEN_USER written by GetTokenInformation, 8-byte aligned; its
    // SID points into the same buffer, which lives to the end of this function.
    let user_sid = sid_to_string(unsafe { (*user_buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid })?;

    let session_buffer = token_info(token, TokenSessionId)?;
    let session_id = first_u32(&session_buffer);

    let label_buffer = token_info(token, TokenIntegrityLevel)?;
    // SAFETY: the buffer holds a TOKEN_MANDATORY_LABEL written by GetTokenInformation, 8-byte
    // aligned; its SID points into the same buffer. The sub-authority count is read before it is
    // used as an index, and a SID without one is refused.
    let integrity_rid = unsafe {
        let sid = (*label_buffer.as_ptr().cast::<TOKEN_MANDATORY_LABEL>())
            .Label
            .Sid;
        let count = *GetSidSubAuthorityCount(sid);
        if count == 0 {
            return Err("the integrity SID has no sub-authority".to_owned());
        }
        *GetSidSubAuthority(sid, u32::from(count) - 1)
    };
    Ok(TokenFacts {
        user_sid,
        session_id,
        integrity_rid,
    })
}

/// Opens a process for the limited queries this module needs.
fn open_limited(pid: u32) -> Result<OwnedHandle, String> {
    // SAFETY: plain OpenProcess with a limited access right; the handle is closed on drop.
    unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
        .map(OwnedHandle)
        .map_err(|e| failed(&format!("OpenProcess {pid}"), &e))
}

fn file_time_ticks(time: FILETIME) -> u64 {
    (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime)
}

/// Facts about a process: identity and token. Everything is read through ONE handle, so the
/// answer is about one process even if the id is reused meanwhile.
///
/// # Errors
/// A message when the process cannot be opened or read (it exited, or access is denied).
pub fn inspect_process(pid: u32) -> Result<ProcessFacts, String> {
    let process = open_limited(pid)?;

    let (mut created, mut exited, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    // SAFETY: a valid process handle with query access and four valid out pointers.
    unsafe {
        GetProcessTimes(
            process.raw(),
            &raw mut created,
            &raw mut exited,
            &raw mut kernel,
            &raw mut user,
        )
    }
    .map_err(|e| failed("GetProcessTimes", &e))?;

    let mut buffer = vec![0_u16; 1024];
    let mut length = u32::try_from(buffer.len()).unwrap_or(1024);
    // SAFETY: `buffer` has `length` elements; the call writes at most that many and updates
    // `length` to the number written, without the terminator.
    unsafe {
        QueryFullProcessImageNameW(
            process.raw(),
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &raw mut length,
        )
    }
    .map_err(|e| failed("QueryFullProcessImageNameW", &e))?;
    let image = String::from_utf16_lossy(&buffer[..length as usize]);

    let mut token = HANDLE::default();
    // SAFETY: a valid process handle and a valid out pointer; the token is closed on drop below.
    unsafe { OpenProcessToken(process.raw(), TOKEN_QUERY, &raw mut token) }
        .map_err(|e| failed("OpenProcessToken", &e))?;
    let token = OwnedHandle(token);

    Ok(ProcessFacts {
        identity: ProcessIdentity {
            pid,
            created_at_100ns: file_time_ticks(created),
            image,
        },
        token: token_facts(token.raw())?,
    })
}

/// Facts about this process.
///
/// # Errors
/// A message when the process's own token cannot be read.
pub fn inspect_current() -> Result<ProcessFacts, String> {
    // SAFETY: no arguments and no preconditions.
    inspect_process(unsafe { GetCurrentProcessId() })
}

/// A process handle that can be compared with the server end of a pipe.
#[derive(Debug)]
pub struct ProcessHandle(OwnedHandle);

impl ProcessHandle {
    /// Opens a process by id with query rights. (The UI side gets its handle from the launch call;
    /// this is for tests and for code that has only an id.)
    ///
    /// # Errors
    /// A message when the process cannot be opened.
    pub fn open(pid: u32) -> Result<Self, String> {
        open_limited(pid).map(Self)
    }

    /// Wraps the process handle a launch call returned. `None` for a null or invalid handle.
    pub(crate) fn from_launch(handle: HANDLE) -> Option<Self> {
        (!handle.is_invalid() && !handle.0.is_null()).then(|| Self(OwnedHandle(handle)))
    }

    /// Whether the process has exited. A handle that cannot be waited on (it lacks the
    /// synchronise right) reports `false`: the caller then relies on the pipe to notice.
    #[must_use]
    pub fn has_exited(&self) -> bool {
        // SAFETY: a valid process handle; a zero timeout only polls.
        unsafe { WaitForSingleObject(self.0.raw(), 0) == WAIT_OBJECT_0 }
    }

    /// The id of the process this handle refers to.
    ///
    /// # Errors
    /// A message when Windows cannot tell.
    pub fn pid(&self) -> Result<u32, String> {
        // SAFETY: a valid process handle with query access.
        match unsafe { GetProcessId(self.0.raw()) } {
            0 => Err(failed("GetProcessId", &windows::core::Error::from_thread())),
            pid => Ok(pid),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token(user: &str, session: u32, rid: u32) -> TokenFacts {
        TokenFacts {
            user_sid: user.to_owned(),
            session_id: session,
            integrity_rid: rid,
        }
    }

    fn identity(pid: u32, created: u64, image: &str) -> ProcessIdentity {
        ProcessIdentity {
            pid,
            created_at_100ns: created,
            image: image.to_owned(),
        }
    }

    fn facts(identity: ProcessIdentity, token: TokenFacts) -> ProcessFacts {
        ProcessFacts { identity, token }
    }

    const UI: &str = r"C:\Program Files\Volt\volt-desktop.exe";

    #[test]
    fn the_bound_process_with_the_right_token_is_served() {
        let own = token("S-1-5-21-1", 1, 0x3000);
        let bound = identity(10, 500, UI);
        let ui = facts(
            identity(10, 500, r"\\?\c:\program files\VOLT\volt-desktop.exe"),
            token("S-1-5-21-1", 1, 0x2000),
        );
        assert_eq!(check_client(&ui, &own, &bound), Ok(()));
        // A UI that is elevated itself (UAC off) is served too: Medium or above.
        let high = facts(identity(10, 500, UI), token("S-1-5-21-1", 1, 0x3000));
        assert_eq!(check_client(&high, &own, &bound), Ok(()));
    }

    #[test]
    fn any_other_process_is_refused() {
        let own = token("S-1-5-21-1", 1, 0x3000);
        let bound = identity(10, 500, UI);
        let user = token("S-1-5-21-1", 1, 0x2000);
        // Another process id, even with the right image and user.
        assert_eq!(
            check_client(&facts(identity(11, 500, UI), user.clone()), &own, &bound),
            Err(ClientRefusal::NotBound)
        );
        // The same id after reuse: another creation time.
        assert_eq!(
            check_client(&facts(identity(10, 501, UI), user.clone()), &own, &bound),
            Err(ClientRefusal::NotBound)
        );
        // The right id and time under another image.
        assert_eq!(
            check_client(
                &facts(identity(10, 500, r"C:\Users\me\evil.exe"), user.clone()),
                &own,
                &bound
            ),
            Err(ClientRefusal::NotBound)
        );
        // A sibling folder with the same file name is not the same image.
        assert_eq!(
            check_client(
                &facts(
                    identity(10, 500, r"C:\Program Files\Volt2\volt-desktop.exe"),
                    user
                ),
                &own,
                &bound
            ),
            Err(ClientRefusal::NotBound)
        );
    }

    #[test]
    fn the_token_rules_are_user_session_and_integrity() {
        let own = token("S-1-5-21-1", 1, 0x3000);
        assert_eq!(
            admit(&token("S-1-5-21-2", 1, 0x2000), &own),
            Err(ClientRefusal::OtherUser)
        );
        assert_eq!(
            admit(&token("S-1-5-21-1", 2, 0x2000), &own),
            Err(ClientRefusal::OtherSession)
        );
        assert_eq!(
            admit(&token("S-1-5-21-1", 1, 0x1000), &own),
            Err(ClientRefusal::LowIntegrity)
        );
        assert_eq!(
            admit(&token("S-1-5-21-1", 1, 0x0000), &own),
            Err(ClientRefusal::LowIntegrity)
        );
        assert_eq!(admit(&token("S-1-5-21-1", 1, 0x2000), &own), Ok(()));
        assert_eq!(admit(&token("S-1-5-21-1", 1, 0x2100), &own), Ok(()));
    }

    #[test]
    fn paths_compare_ignoring_case_prefix_and_slashes() {
        assert_eq!(
            normalise_path(r"\\?\C:\Spike\Bin\Helper.EXE"),
            normalise_path("c:/spike/bin/helper.exe")
        );
        assert_ne!(
            normalise_path(r"C:\spike\bin\helper.exe"),
            normalise_path(r"C:\spike\bin\other\helper.exe")
        );
    }

    #[test]
    fn this_process_can_be_inspected() {
        let me = inspect_current().expect("own process");
        assert_eq!(me.identity.pid, std::process::id());
        assert!(me.identity.created_at_100ns > 0);
        let exe = std::env::current_exe().expect("exe");
        assert_eq!(
            normalise_path(&me.identity.image),
            normalise_path(&exe.to_string_lossy())
        );
        assert!(me.token.user_sid.starts_with("S-1-5-"));
        assert!(me.token.integrity_rid >= MEDIUM_INTEGRITY_RID);
        // A second read gives the same identity.
        assert_eq!(
            inspect_process(me.identity.pid).expect("again").identity,
            me.identity
        );
        // The process is bound to itself and to nothing else.
        assert!(me.identity.is_bound_process(&me.identity));
        assert!(!me.identity.is_bound_process(&ProcessIdentity {
            created_at_100ns: me.identity.created_at_100ns + 1,
            ..me.identity.clone()
        }));
        // Admission of a process with its own token always passes (same user and session).
        assert_eq!(admit(&me.token, &me.token), Ok(()));
    }

    #[test]
    fn a_missing_process_cannot_be_inspected() {
        // Process ids are multiples of four; this one is not and cannot exist.
        assert!(inspect_process(0x7FFF_FFFF - 1).is_err());
    }

    #[test]
    fn a_token_is_classified_by_elevation_and_type() {
        assert_eq!(classify_token(true, 2), TokenKind::Elevated);
        // UAC off: elevated with the default type.
        assert_eq!(classify_token(true, 1), TokenKind::Elevated);
        assert_eq!(classify_token(false, 3), TokenKind::Limited);
        assert_eq!(classify_token(false, 1), TokenKind::Standard);
        assert_eq!(classify_token(false, 0), TokenKind::Standard);
    }

    #[test]
    fn this_process_token_can_be_classified() {
        assert!(current_token_kind().is_ok());
    }

    #[test]
    fn a_process_handle_reports_its_process() {
        let handle = ProcessHandle::open(std::process::id()).expect("open");
        assert_eq!(handle.pid(), Ok(std::process::id()));
        // This process is running.
        assert!(!handle.has_exited());
    }
}
