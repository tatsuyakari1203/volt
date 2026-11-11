//! The helper's own state folder and the single-helper lock.
//!
//! The helper keeps its journal, activity log and settings in `<install folder>\state` (see the
//! decision on the helper). Everything in it decides what an undo restores, so it must be a place
//! only administrators can write:
//!
//! - the helper CREATES the folder when it is missing, with a protected ACL (SYSTEM and
//!   Administrators only, nothing inherited) so that what is in it never depends on what the
//!   parent folder allows;
//! - whether it was created now or earlier (by the installer, or by an earlier helper), it is then
//!   checked like the rest of the install: not a reparse point, owned by SYSTEM, Administrators or
//!   `TrustedInstaller`, and no entry that lets anyone else write, delete or re-permission it. A
//!   folder that fails the check is never used ([`StateError::Refused`]);
//! - one folder per user inside it (`state\<user SID>`), so a user's history is served to that
//!   user only;
//! - a lock file, `state\helper.lock`, opened WITHOUT sharing, so a second helper on the machine
//!   cannot open it and refuses to run ([`LockError::Held`]). The lock is the open handle: it goes
//!   when the helper ends, however it ends, so a killed helper never leaves a stale lock.

use std::fs::File;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::Storage::FileSystem::{CreateDirectoryW, FILE_FLAG_OPEN_REPARSE_POINT};
use windows::core::PCWSTR;

use super::acl;
use super::install::{InstallLayout, InstallRefusal, check_security, verify_tree};
use super::{failed, wide};

/// The lock file inside the state folder.
pub const LOCK_FILE_NAME: &str = "helper.lock";

/// The ACL of a state folder the helper creates: owned by Administrators, full control for SYSTEM
/// and Administrators, inherited by what is created inside, and PROTECTED (`P`), so nothing is
/// inherited from the install folder.
pub const PROTECTED_SDDL: &str = "O:BAD:PAI(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)";

const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
const ERROR_SHARING_VIOLATION: i32 = 32;
const ERROR_LOCK_VIOLATION: i32 = 33;

/// Why the state folder cannot be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateError {
    /// The folder, or a folder in it, can be changed by a non-administrator, is a link, or could
    /// not be checked.
    Refused(InstallRefusal),
    /// The folder could not be created or read.
    Io(String),
    /// The user's SID is not the text of a SID, so it must not become a folder name.
    BadSid(String),
}

impl std::fmt::Display for StateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused(refusal) => write!(f, "the state folder is refused: {refusal}"),
            Self::Io(reason) => write!(f, "the state folder cannot be used: {reason}"),
            Self::BadSid(sid) => write!(f, "{sid:?} is not a user SID"),
        }
    }
}

impl std::error::Error for StateError {}

/// A security descriptor made from SDDL text, freed on drop.
pub(crate) struct Descriptor(PSECURITY_DESCRIPTOR);

impl Descriptor {
    pub(crate) fn raw(&self) -> PSECURITY_DESCRIPTOR {
        self.0
    }

    pub(crate) fn parse(sddl: &str) -> Result<Self, String> {
        let sddl = wide(sddl);
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        // SAFETY: `sddl` is NUL-terminated and outlives the call; `descriptor` is a valid out
        // pointer. The allocated descriptor is owned by the returned value and freed on drop.
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(sddl.as_ptr()),
                SDDL_REVISION_1,
                &raw mut descriptor,
                None,
            )
        }
        .map_err(|e| failed("ConvertStringSecurityDescriptorToSecurityDescriptorW", &e))?;
        Ok(Self(descriptor))
    }
}

impl Drop for Descriptor {
    fn drop(&mut self) {
        // SAFETY: the descriptor was allocated by the conversion call and is freed once, here.
        let _ = unsafe { LocalFree(Some(HLOCAL(self.0.0))) };
    }
}

/// Creates `path` with the security descriptor written as `sddl`. An existing folder is left as it
/// is (and is not an error).
///
/// # Errors
/// A message when the descriptor or the folder cannot be made.
pub fn create_dir_with_sddl(path: &Path, sddl: &str) -> Result<(), String> {
    let descriptor = Descriptor::parse(sddl)?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: u32::try_from(std::mem::size_of::<SECURITY_ATTRIBUTES>()).unwrap_or(0),
        lpSecurityDescriptor: descriptor.0.0,
        bInheritHandle: false.into(),
    };
    let name = wide(&path.to_string_lossy());
    // SAFETY: `name` is NUL-terminated and `attributes` points at the valid descriptor above; both
    // outlive the call, which copies the descriptor.
    let created = unsafe { CreateDirectoryW(PCWSTR(name.as_ptr()), Some(&raw const attributes)) };
    match created {
        Ok(()) => Ok(()),
        Err(error) if error.code() == ERROR_ALREADY_EXISTS.to_hresult() => Ok(()),
        Err(error) => Err(failed("CreateDirectoryW", &error)),
    }
}

/// Makes sure the state folder exists and is one that only administrators can write.
///
/// # Errors
/// [`StateError::Refused`] when the folder (or the install around it) fails the check,
/// [`StateError::Io`] when it cannot be created.
pub fn prepare_state_dir(layout: &InstallLayout) -> Result<(), StateError> {
    match std::fs::symlink_metadata(&layout.state_dir) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            create_dir_with_sddl(&layout.state_dir, PROTECTED_SDDL).map_err(StateError::Io)?;
        }
        Err(error) => return Err(StateError::Io(error.to_string())),
    }
    verify_tree(layout).map_err(StateError::Refused)
}

/// The folder for one user's journal and log, below the state folder: `<state>\<SID>`. It is
/// created when missing, inherits the protected ACL of its parent, and is checked like it.
///
/// # Errors
/// [`StateError::BadSid`] for text that is not a SID, otherwise as [`prepare_state_dir`].
pub fn user_dir(layout: &InstallLayout, user_sid: &str) -> Result<PathBuf, StateError> {
    if !looks_like_sid(user_sid) {
        return Err(StateError::BadSid(user_sid.to_owned()));
    }
    let dir = layout.state_dir.join(user_sid);
    // Made like the state folder (an existing folder is left as it is), so its owner and its rights
    // do not depend on the account that happens to run the helper.
    create_dir_with_sddl(&dir, PROTECTED_SDDL).map_err(StateError::Io)?;
    verify_folder(&dir).map_err(StateError::Refused)?;
    Ok(dir)
}

/// A SID as text (`S-1-5-21-...`): letters and digits and hyphens only, so it can be a folder name.
fn looks_like_sid(text: &str) -> bool {
    text.len() <= 184
        && text.starts_with("S-1-")
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || b == b'-' || b == b'S')
}

/// The reparse-point and security check of one folder.
fn verify_folder(path: &Path) -> Result<(), InstallRefusal> {
    let unreadable = |reason: String| InstallRefusal::Unreadable {
        path: path.to_path_buf(),
        reason,
    };
    let metadata = std::fs::symlink_metadata(path).map_err(|e| unreadable(e.to_string()))?;
    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(InstallRefusal::ReparsePoint(path.to_path_buf()));
    }
    let info = acl::of_path(path).map_err(unreadable)?;
    check_security(&info).map_err(|weakness| InstallRefusal::Weak {
        path: path.to_path_buf(),
        weakness,
    })
}

/// Why the lock could not be taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockError {
    /// Another helper holds it.
    Held,
    /// The lock file could not be opened for another reason.
    Io(String),
}

impl std::fmt::Display for LockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Held => f.write_str("another helper is running"),
            Self::Io(reason) => write!(f, "the lock file cannot be opened: {reason}"),
        }
    }
}

impl std::error::Error for LockError {}

/// The single-helper lock: the open, unshared handle of `state\helper.lock`. Dropping it, or the
/// process ending, releases it.
#[derive(Debug)]
pub struct StateLock {
    _file: File,
}

impl StateLock {
    /// Takes the lock in `state_dir`.
    ///
    /// # Errors
    /// [`LockError::Held`] when another helper has it, [`LockError::Io`] otherwise.
    pub fn acquire(state_dir: &Path) -> Result<Self, LockError> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            // Nobody else may open it while this handle lives. A link is opened as itself, not
            // followed.
            .share_mode(0)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
            .open(state_dir.join(LOCK_FILE_NAME))
            .map_err(|error| match error.raw_os_error() {
                Some(ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION) => LockError::Held,
                _ => LockError::Io(error.to_string()),
            })?;
        Ok(Self { _file: file })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::helper::acl::of_path;
    use crate::helper::identity::inspect_current;

    #[test]
    fn a_second_lock_is_refused_until_the_first_is_released() {
        let dir = tempfile::tempdir().expect("temp");
        let first = StateLock::acquire(dir.path()).expect("the first lock");
        assert_eq!(
            StateLock::acquire(dir.path()).err(),
            Some(LockError::Held),
            "a second helper must not run"
        );
        drop(first);
        let again = StateLock::acquire(dir.path()).expect("free again");
        drop(again);
        // The file stays: only the handle is the lock.
        assert!(dir.path().join(LOCK_FILE_NAME).exists());
    }

    #[test]
    fn a_lock_in_a_folder_that_does_not_exist_is_an_io_error() {
        let dir = tempfile::tempdir().expect("temp");
        let missing = dir.path().join("no-such-folder");
        assert!(matches!(
            StateLock::acquire(&missing),
            Err(LockError::Io(_))
        ));
    }

    #[test]
    fn only_a_sid_can_become_a_folder_name() {
        assert!(looks_like_sid(
            "S-1-5-21-1004336348-1177238915-682003330-1001"
        ));
        assert!(looks_like_sid("S-1-5-18"));
        for bad in [
            "",
            "..",
            r"S-1-5\..\x",
            "S-1-5-21/x",
            "S-1-5-21-1:stream",
            "s-1-5-18",
            "C:",
            "S-1-5-18 ",
        ] {
            assert!(!looks_like_sid(bad), "{bad:?}");
        }
        let layout = InstallLayout::around(Path::new(r"C:\Program Files\Volt\volt-helper.exe"))
            .expect("layout");
        assert!(matches!(
            user_dir(&layout, r"..\x"),
            Err(StateError::BadSid(_))
        ));
    }

    #[test]
    fn the_protected_acl_of_the_state_folder_is_valid_sddl() {
        // Parses only: a folder with this ACL could not be removed by an ordinary user, so the
        // elevated CI job creates the real one on a hosted runner.
        assert!(Descriptor::parse(PROTECTED_SDDL).is_ok());
        assert!(Descriptor::parse("not sddl").is_err());
    }

    #[test]
    fn a_folder_made_with_a_protected_acl_inherits_nothing_and_keeps_only_what_was_written() {
        let me = inspect_current().expect("token").token.user_sid;
        let dir = tempfile::tempdir().expect("temp");
        let target = dir.path().join("protected");
        let sddl = format!("D:PAI(A;OICI;FA;;;{me})");
        create_dir_with_sddl(&target, &sddl).expect("created");
        let info = of_path(&target).expect("readable");
        let aces = info.dacl.expect("a DACL");
        assert_eq!(aces.len(), 1, "{aces:?}");
        assert_eq!(aces[0].sid, me);
        // 0x10 is the inherited flag: a protected folder has none from its parent.
        assert!(aces.iter().all(|ace| ace.flags & 0x10 == 0), "{aces:?}");
        // Making it again is not an error and changes nothing.
        create_dir_with_sddl(&target, &sddl).expect("already there");
    }

    #[test]
    fn a_state_folder_inside_a_folder_the_user_can_write_is_refused() {
        let dir = tempfile::tempdir().expect("temp");
        let install = dir.path().join("Volt");
        std::fs::create_dir_all(install.join("state")).expect("folders");
        let image = install.join("volt-helper.exe");
        std::fs::write(&image, b"not a program").expect("image");
        let layout = InstallLayout::around(&image).expect("layout");
        assert!(matches!(
            prepare_state_dir(&layout),
            Err(StateError::Refused(InstallRefusal::Weak { .. }))
        ));
    }

    #[test]
    fn a_folder_the_user_made_inside_the_state_folder_is_refused() {
        let dir = tempfile::tempdir().expect("temp");
        let layout = InstallLayout {
            install_dir: dir.path().to_path_buf(),
            image: dir.path().join("volt-helper.exe"),
            state_dir: dir.path().to_path_buf(),
        };
        // The folder exists already, made by the user: `user_dir` finds it weak. The real state
        // folder is administrator-only, so a user cannot make one and the check is a tripwire.
        let sid = inspect_current().expect("token").token.user_sid;
        std::fs::create_dir(dir.path().join(&sid)).expect("the user's own folder");
        assert!(matches!(
            user_dir(&layout, &sid),
            Err(StateError::Refused(InstallRefusal::Weak { .. }))
        ));
    }
}
