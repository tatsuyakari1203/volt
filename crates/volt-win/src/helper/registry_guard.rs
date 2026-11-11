//! The registry the elevated helper writes through: [`AdminRegistry`].
//!
//! An administrator tweak changes machine-wide keys with administrator rights. If any key on the
//! way to the value can be written by a non-administrator, that program can swap a subkey for a
//! symbolic link and steer the helper's write somewhere else, or plant the key first. So before
//! every write the helper walks the whole path, key by key from the hive, and requires of EVERY
//! key on it (see the decision on the helper):
//!
//! - it is not a symbolic link (the key is opened as a link, never followed, and a link key is
//!   recognised by its `SymbolicLinkValue`);
//! - its owner is SYSTEM, Administrators or `TrustedInstaller`;
//! - no entry of its DACL grants a right to change it (set a value, create a subkey or a link,
//!   delete, change the DACL or the owner) to anyone else.
//!
//! The checks read the OPEN handles, one level below the previous handle, so a key cannot be
//! exchanged between the check and the open. A key that does not exist yet ends the walk: it will be
//! created below a parent that was just checked, and it inherits the parent's rights. Nothing that
//! cannot be read is let through.
//!
//! Only `HKLM` is written: the per-user hives are the user's, and the helper never touches them.
//! Reads are not guarded.

use volt_core::{
    NamedValue, PortError, Registry, RegistryHive, RegistryKeyRef, RegistryValue, RegistryValueRef,
    RegistryView,
};
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, ERROR_SUCCESS};
use windows::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_QUERY_VALUE, KEY_WOW64_32KEY, KEY_WOW64_64KEY, REG_LINK,
    REG_OPTION_OPEN_LINK, REG_SAM_FLAGS, REG_VALUE_TYPE, RegCloseKey, RegOpenKeyExW,
    RegQueryValueExW,
};
use windows::core::PCWSTR;

use super::acl::{self, SecurityInfo};
use super::install::{KEY_WRITE_RIGHTS, Weakness, check_security_for};
use crate::registry::WinRegistry;
use crate::wide::to_wide_nul;

/// `READ_CONTROL`: read the owner and the DACL.
const READ_CONTROL: u32 = 0x0002_0000;

/// The value that makes a key a symbolic link.
const LINK_VALUE: &str = "SymbolicLinkValue";

/// Why a registry path is refused for writing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyRefusal {
    /// The hive is not `HKLM`.
    NotMachineHive(RegistryHive),
    /// The path is empty or has an empty or malformed part.
    BadPath(String),
    /// A key on the path is a symbolic link.
    SymbolicLink {
        /// The key, from the hive.
        path: String,
    },
    /// A key on the path can be changed by someone who is not an administrator, or is owned by
    /// such a one.
    Weak {
        /// The key, from the hive.
        path: String,
        /// What is wrong with it.
        weakness: Weakness,
    },
    /// A key on the path could not be inspected. Not knowing is a refusal.
    Unreadable {
        /// The key, from the hive.
        path: String,
        /// Why.
        reason: String,
    },
}

impl std::fmt::Display for KeyRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotMachineHive(hive) => write!(f, "only HKLM may be changed here, not {hive:?}"),
            Self::BadPath(reason) => write!(f, "not a registry key path ({reason})"),
            Self::SymbolicLink { path } => write!(f, "{path} is a symbolic link"),
            Self::Weak { path, weakness } => match weakness {
                Weakness::UntrustedOwner(owner) => {
                    write!(f, "{path} is owned by {owner}, not by an administrator")
                }
                Weakness::Writable { sid, .. } => {
                    write!(
                        f,
                        "{path} can be changed by {sid}, not only by administrators"
                    )
                }
            },
            Self::Unreadable { path, reason } => {
                write!(f, "{path} cannot be checked ({reason})")
            }
        }
    }
}

impl std::error::Error for KeyRefusal {}

/// Judges one key from what was read off its open handle. Pure, so the rules are tested with
/// made-up lists.
///
/// # Errors
/// [`KeyRefusal::SymbolicLink`] (checked first: a link's own rights say nothing about where it
/// leads), then [`KeyRefusal::Weak`].
pub fn judge_key(path: &str, is_link: bool, info: &SecurityInfo) -> Result<(), KeyRefusal> {
    if is_link {
        return Err(KeyRefusal::SymbolicLink {
            path: path.to_owned(),
        });
    }
    // The CREATOR OWNER and CREATOR GROUP entries are placeholders that become the creator when a
    // subkey is made: no process token holds them, so on an existing key they grant nothing, and
    // the stock machine keys carry them with full control.
    let mut judged = info.clone();
    if let Some(aces) = &mut judged.dacl {
        aces.retain(|ace| !matches!(ace.sid.as_str(), "S-1-3-0" | "S-1-3-1"));
    }
    check_security_for(&judged, KEY_WRITE_RIGHTS).map_err(|weakness| KeyRefusal::Weak {
        path: path.to_owned(),
        weakness,
    })
}

/// An open key, closed on drop.
struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful RegOpenKeyExW, is owned by this value and is
        // closed exactly once, here.
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

fn view_flags(view: RegistryView) -> REG_SAM_FLAGS {
    match view {
        RegistryView::Native => REG_SAM_FLAGS(0),
        RegistryView::Bit32 => KEY_WOW64_32KEY,
        RegistryView::Bit64 => KEY_WOW64_64KEY,
    }
}

/// What was read off one key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KeyFacts {
    pub(crate) is_link: bool,
    pub(crate) security: SecurityInfo,
}

/// Opens the key `name` below `parent` WITHOUT following a link and reads its facts. `Ok(None)` when
/// the key does not exist.
fn open_and_read(
    parent: HKEY,
    name: &str,
    view: RegistryView,
) -> Result<Option<(Key, KeyFacts)>, String> {
    let wide = to_wide_nul(name, "registry key name").map_err(|error| error.to_string())?;
    let mut handle = HKEY::default();
    // SAFETY: `wide` is NUL-terminated UTF-16 outliving the call, `parent` is an open key or a
    // predefined key, and `handle` is a valid out pointer.
    let status = unsafe {
        RegOpenKeyExW(
            parent,
            PCWSTR(wide.as_ptr()),
            Some(REG_OPTION_OPEN_LINK.0),
            REG_SAM_FLAGS(KEY_QUERY_VALUE.0 | READ_CONTROL) | view_flags(view),
            &raw mut handle,
        )
    };
    if status == ERROR_FILE_NOT_FOUND || status == ERROR_PATH_NOT_FOUND {
        return Ok(None);
    }
    if status != ERROR_SUCCESS {
        return Err(format!("open: Win32 error {}", status.0));
    }
    let key = Key(handle);
    let is_link = is_link(&key)?;
    let security = acl::of_registry_key(key.0)?;
    Ok(Some((key, KeyFacts { is_link, security })))
}

/// Whether an open key is a symbolic link: it holds a `SymbolicLinkValue` of type `REG_LINK`.
fn is_link(key: &Key) -> Result<bool, String> {
    let name = to_wide_nul(LINK_VALUE, "value name").map_err(|error| error.to_string())?;
    let mut kind = REG_VALUE_TYPE(0);
    // SAFETY: `key` is open with KEY_QUERY_VALUE and `name` is NUL-terminated UTF-16 outliving the
    // call; only the type is asked for (no data buffer), through a valid out pointer.
    let status = unsafe {
        RegQueryValueExW(
            key.0,
            PCWSTR(name.as_ptr()),
            None,
            Some(&raw mut kind),
            None,
            None,
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(false);
    }
    // Any other answer is read as the link value being there: a key that is not a link does not
    // have it, and a key that claims to be one is refused. An error that is not "not found" is
    // refused as well.
    if status == ERROR_SUCCESS {
        return Ok(kind == REG_LINK);
    }
    Err(format!("read the link value: Win32 error {}", status.0))
}

/// Walks `subkey` below `root` (a predefined or open key; `label` names it in messages), checking
/// every key that exists with [`judge_key`].
///
/// # Errors
/// The first [`KeyRefusal`].
pub(crate) fn walk(
    root: HKEY,
    label: &str,
    subkey: &str,
    view: RegistryView,
) -> Result<(), KeyRefusal> {
    if subkey.is_empty() {
        return Err(KeyRefusal::BadPath("the path is empty".to_owned()));
    }
    let mut parent: Option<Key> = None;
    let mut path = label.to_owned();
    for component in subkey.split('\\') {
        if component.is_empty() {
            return Err(KeyRefusal::BadPath("an empty part".to_owned()));
        }
        path.push('\\');
        path.push_str(component);
        let at = parent.as_ref().map_or(root, |key| key.0);
        let found =
            open_and_read(at, component, view).map_err(|reason| KeyRefusal::Unreadable {
                path: path.clone(),
                reason,
            })?;
        let Some((key, facts)) = found else {
            // The rest does not exist yet: it is created below a key that was just checked.
            return Ok(());
        };
        judge_key(&path, facts.is_link, &facts.security)?;
        parent = Some(key);
    }
    Ok(())
}

/// Checks the path of a key that is about to be written below `hive`.
///
/// # Errors
/// The first [`KeyRefusal`]: another hive than `HKLM`, a symbolic link, a key a non-administrator
/// can write, or a key that cannot be inspected.
pub fn check_key_path(
    hive: RegistryHive,
    subkey: &str,
    view: RegistryView,
) -> Result<(), KeyRefusal> {
    if hive != RegistryHive::LocalMachine {
        return Err(KeyRefusal::NotMachineHive(hive));
    }
    walk(HKEY_LOCAL_MACHINE, "HKLM", subkey, view)
}

/// The registry port of the elevated helper: reads like [`WinRegistry`], writes only after
/// [`check_key_path`] has passed for the whole path.
#[derive(Debug, Default, Clone, Copy)]
pub struct AdminRegistry {
    inner: WinRegistry,
}

impl AdminRegistry {
    /// A guarded registry.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            inner: WinRegistry::new(),
        }
    }
}

/// The error a refused path becomes at the port: access denied, so the executor records a failed
/// step and changes nothing.
fn refused(refusal: &KeyRefusal) -> PortError {
    match refusal {
        KeyRefusal::BadPath(reason) => PortError::InvalidInput(reason.clone()),
        other => PortError::AccessDenied(format!("refused: {other}")),
    }
}

impl Registry for AdminRegistry {
    fn list_values(&self, key: &RegistryKeyRef) -> Result<Vec<NamedValue>, PortError> {
        self.inner.list_values(key)
    }

    fn read_value(&self, target: &RegistryValueRef) -> Result<Option<RegistryValue>, PortError> {
        self.inner.read_value(target)
    }

    fn write_value(
        &self,
        target: &RegistryValueRef,
        data: &RegistryValue,
    ) -> Result<(), PortError> {
        check_key_path(target.hive, &target.subkey, target.view).map_err(|r| refused(&r))?;
        self.inner.write_value(target, data)
    }

    fn delete_value(&self, target: &RegistryValueRef) -> Result<(), PortError> {
        check_key_path(target.hive, &target.subkey, target.view).map_err(|r| refused(&r))?;
        self.inner.delete_value(target)
    }

    fn list_subkeys(&self, key: &RegistryKeyRef) -> Result<Vec<String>, PortError> {
        self.inner.list_subkeys(key)
    }

    fn create_key(&self, key: &RegistryKeyRef) -> Result<(), PortError> {
        check_key_path(key.hive, &key.subkey, key.view).map_err(|r| refused(&r))?;
        self.inner.create_key(key)
    }

    fn delete_key(&self, key: &RegistryKeyRef) -> Result<(), PortError> {
        check_key_path(key.hive, &key.subkey, key.view).map_err(|r| refused(&r))?;
        self.inner.delete_key(key)
    }

    fn key_exists(
        &self,
        hive: RegistryHive,
        subkey: &str,
        view: RegistryView,
    ) -> Result<bool, PortError> {
        self.inner.key_exists(hive, subkey, view)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use windows::Win32::System::Registry::{
        HKEY_CURRENT_USER, KEY_ALL_ACCESS, REG_OPEN_CREATE_OPTIONS, REG_OPTION_CREATE_LINK,
        REG_OPTION_NON_VOLATILE, RegCreateKeyExW, RegDeleteTreeW, RegSetValueExW,
    };

    use super::super::acl::Ace;
    use super::super::identity::inspect_current;
    use super::*;

    const ADMINS: &str = "S-1-5-32-544";
    const SYSTEM: &str = "S-1-5-18";
    const USER: &str = "S-1-5-21-1-2-3-1001";
    const READ: u32 = 0x0002_0019; // KEY_READ
    const FULL: u32 = 0x000f_003f; // KEY_ALL_ACCESS

    fn info(owner: &str, aces: &[(bool, &str, u32, u8)]) -> SecurityInfo {
        SecurityInfo {
            owner: owner.to_owned(),
            dacl: Some(
                aces.iter()
                    .map(|(allow, sid, mask, flags)| Ace {
                        allow: *allow,
                        sid: (*sid).to_owned(),
                        mask: *mask,
                        flags: *flags,
                    })
                    .collect(),
            ),
        }
    }

    fn stock() -> SecurityInfo {
        info(
            ADMINS,
            &[
                (true, ADMINS, FULL, 0),
                (true, SYSTEM, FULL, 0),
                (true, "S-1-5-32-545", READ, 0),
                // CREATOR OWNER: a placeholder, full control on this key and below, as on the
                // real HKLM\SOFTWARE.
                (true, "S-1-3-0", FULL, 0x02),
            ],
        )
    }

    #[test]
    fn a_stock_machine_key_passes() {
        assert_eq!(judge_key(r"HKLM\SOFTWARE", false, &stock()), Ok(()));
    }

    #[test]
    fn a_symbolic_link_is_refused_whatever_its_rights() {
        assert_eq!(
            judge_key(r"HKLM\SOFTWARE\Link", true, &stock()),
            Err(KeyRefusal::SymbolicLink {
                path: r"HKLM\SOFTWARE\Link".to_owned()
            })
        );
    }

    #[test]
    fn each_right_to_change_a_key_is_refused_for_a_non_administrator() {
        for (name, mask) in [
            ("set value", 0x2),
            ("create subkey", 0x4),
            ("create link", 0x20),
            ("delete", 0x0001_0000),
            ("change permissions", 0x0004_0000),
            ("take ownership", 0x0008_0000),
            ("generic all", 0x1000_0000),
            ("generic write", 0x4000_0000),
            ("key all access", FULL),
        ] {
            let mut weak = stock();
            if let Some(aces) = &mut weak.dacl {
                aces.push(Ace {
                    allow: true,
                    sid: USER.to_owned(),
                    mask: mask | READ,
                    flags: 0,
                });
            }
            assert!(
                matches!(
                    judge_key(r"HKLM\SOFTWARE\Key", false, &weak),
                    Err(KeyRefusal::Weak {
                        weakness: Weakness::Writable { .. },
                        ..
                    })
                ),
                "{name}"
            );
        }
    }

    #[test]
    fn rights_that_only_read_a_key_are_fine_for_anyone() {
        let mut readable = stock();
        if let Some(aces) = &mut readable.dacl {
            // KEY_READ includes KEY_NOTIFY (0x10) and the enumeration rights, which share a
            // position with the FILE write-EA right: not a way to change a key.
            aces.push(Ace {
                allow: true,
                sid: USER.to_owned(),
                mask: 0x0002_0019 | 0x10,
                flags: 0,
            });
        }
        assert_eq!(judge_key(r"HKLM\SOFTWARE\Key", false, &readable), Ok(()));
    }

    #[test]
    fn a_key_owned_by_a_user_is_refused_even_with_a_clean_list() {
        let owned = info(USER, &[(true, ADMINS, FULL, 0), (true, SYSTEM, FULL, 0)]);
        assert_eq!(
            judge_key(r"HKLM\SOFTWARE\Key", false, &owned),
            Err(KeyRefusal::Weak {
                path: r"HKLM\SOFTWARE\Key".to_owned(),
                weakness: Weakness::UntrustedOwner(USER.to_owned())
            })
        );
    }

    #[test]
    fn a_null_dacl_is_refused() {
        let null = SecurityInfo {
            owner: ADMINS.to_owned(),
            dacl: None,
        };
        assert!(judge_key(r"HKLM\SOFTWARE\Key", false, &null).is_err());
    }

    #[test]
    fn only_hklm_is_written() {
        for hive in [
            RegistryHive::CurrentUser,
            RegistryHive::Users,
            RegistryHive::ClassesRoot,
            RegistryHive::CurrentConfig,
        ] {
            assert_eq!(
                check_key_path(hive, r"Software\Example", RegistryView::Native),
                Err(KeyRefusal::NotMachineHive(hive))
            );
        }
    }

    #[test]
    fn a_malformed_path_is_refused() {
        for subkey in ["", r"\SOFTWARE", r"SOFTWARE\", r"SOFTWARE\\Microsoft"] {
            assert!(
                matches!(
                    check_key_path(RegistryHive::LocalMachine, subkey, RegistryView::Native),
                    Err(KeyRefusal::BadPath(_))
                ),
                "{subkey:?}"
            );
        }
    }

    #[test]
    fn a_real_machine_key_passes_and_a_path_that_does_not_exist_yet_ends_the_walk() {
        // Reads only: the real ACLs of keys that every Windows has.
        assert_eq!(
            check_key_path(
                RegistryHive::LocalMachine,
                r"SOFTWARE\Microsoft",
                RegistryView::Native
            ),
            Ok(())
        );
        assert_eq!(
            check_key_path(
                RegistryHive::LocalMachine,
                r"SOFTWARE\VoltTestNoSuchKey\Below\It",
                RegistryView::Native
            ),
            Ok(())
        );
    }

    #[test]
    fn the_guarded_port_writes_nothing_outside_hklm() {
        // A real HKCU value under the throwaway test key: the port must refuse before any write.
        let target = RegistryValueRef::new(
            RegistryHive::CurrentUser,
            r"Software\VoltTest\Guarded",
            "Value",
        );
        let port = AdminRegistry::new();
        let refused = port.write_value(&target, &RegistryValue::Dword(1));
        assert!(
            matches!(refused, Err(PortError::AccessDenied(_))),
            "{refused:?}"
        );
        assert!(matches!(
            port.create_key(&RegistryKeyRef::new(
                RegistryHive::CurrentUser,
                r"Software\VoltTest\Guarded",
                RegistryView::Native
            )),
            Err(PortError::AccessDenied(_))
        ));
        assert_eq!(WinRegistry::new().read_value(&target), Ok(None));
        assert_eq!(
            port.key_exists(
                RegistryHive::CurrentUser,
                r"Software\VoltTest\Guarded",
                RegistryView::Native
            ),
            Ok(false)
        );
    }

    unsafe extern "system" {
        /// Deletes the key the handle names (a link key too, when it was opened as a link).
        fn NtDeleteKey(key: HKEY) -> i32;
    }

    /// Deletes the symbolic link `name` below `parent` itself: it is opened as a link (never
    /// followed) and deleted through its handle, which neither `RegDeleteKey` nor `RegDeleteTree`
    /// do for a link.
    fn delete_link(parent: HKEY, name: &str) {
        let wide = to_wide_nul(name, "key").unwrap_or_default();
        let mut handle = HKEY::default();
        // SAFETY: NUL-terminated UTF-16 outliving the call and a valid out pointer; `parent` is an
        // open key of the test; DELETE (0x10000) is the only right asked for.
        let status = unsafe {
            RegOpenKeyExW(
                parent,
                PCWSTR(wide.as_ptr()),
                Some(REG_OPTION_OPEN_LINK.0),
                REG_SAM_FLAGS(0x0001_0000),
                &raw mut handle,
            )
        };
        if status == ERROR_SUCCESS {
            // SAFETY: `handle` is the link key just opened with DELETE; it is closed once.
            unsafe {
                let _ = NtDeleteKey(handle);
                let _ = RegCloseKey(handle);
            }
        }
    }

    /// A unique key under `HKCU\Software\VoltTest`, removed with everything below it on drop.
    struct TestKey {
        subkey: String,
        handle: HKEY,
        /// Links made below it, deleted as links before the tree goes.
        links: std::sync::Mutex<Vec<String>>,
    }

    impl TestKey {
        fn new() -> Self {
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos());
            let subkey = format!(
                r"Software\VoltTest\guard-{}-{nanos}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            );
            let handle = create(HKEY_CURRENT_USER, &subkey, false).expect("test key");
            Self {
                subkey,
                handle,
                links: std::sync::Mutex::new(Vec::new()),
            }
        }
    }

    impl Drop for TestKey {
        fn drop(&mut self) {
            let links = self
                .links
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            for link in links {
                delete_link(self.handle, &link);
            }
            let wide = to_wide_nul(&self.subkey, "key").unwrap_or_default();
            // SAFETY: the handle is the one `new` opened and is closed once; the path is a
            // NUL-terminated UTF-16 buffer naming the unique key this test made below
            // HKEY_CURRENT_USER, the only key deleted.
            unsafe {
                let _ = RegCloseKey(self.handle);
                let _ = RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(wide.as_ptr()));
            }
        }
    }

    /// Creates (or opens) `name` below `parent`, as a symbolic link when `link` is set.
    fn create(parent: HKEY, name: &str, link: bool) -> Result<HKEY, String> {
        let wide = to_wide_nul(name, "key").map_err(|e| e.to_string())?;
        let options = if link {
            REG_OPEN_CREATE_OPTIONS(REG_OPTION_CREATE_LINK.0 | REG_OPTION_NON_VOLATILE.0)
        } else {
            REG_OPTION_NON_VOLATILE
        };
        let mut handle = HKEY::default();
        // SAFETY: NUL-terminated UTF-16 outliving the call, optional arguments as None/null, a
        // valid out pointer; the key is below HKCU and the test's own.
        let status = unsafe {
            RegCreateKeyExW(
                parent,
                PCWSTR(wide.as_ptr()),
                None,
                PCWSTR::null(),
                options,
                KEY_ALL_ACCESS,
                None,
                &raw mut handle,
                None,
            )
        };
        if status == ERROR_SUCCESS {
            Ok(handle)
        } else {
            Err(format!("create {name}: Win32 error {}", status.0))
        }
    }

    /// Makes `link` (below `root`) a symbolic link to the key `target` (an absolute kernel path).
    fn make_link(key: &TestKey, link: &str, target: &str) {
        key.links
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(link.to_owned());
        let handle = create(key.handle, link, true).expect("link key");
        let units: Vec<u8> = target.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let name = to_wide_nul(LINK_VALUE, "value").expect("name");
        // SAFETY: `handle` is the link key just created (full access), `name` is NUL-terminated
        // UTF-16 and `units` is a byte slice (without a terminator, as REG_LINK data is stored)
        // that outlives the call.
        let status =
            unsafe { RegSetValueExW(handle, PCWSTR(name.as_ptr()), None, REG_LINK, Some(&units)) };
        // SAFETY: the handle was opened above and is closed once.
        unsafe {
            let _ = RegCloseKey(handle);
        }
        assert_eq!(status, ERROR_SUCCESS, "set the link value");
    }

    #[test]
    fn a_plain_user_key_is_refused_by_its_real_owner_and_rights() {
        let key = TestKey::new();
        let sid = inspect_current().expect("token").token.user_sid;
        // The walk starts at HKCU here: the first key, `Software`, belongs to the user.
        let refused = walk(HKEY_CURRENT_USER, "HKCU", &key.subkey, RegistryView::Native);
        match refused {
            Err(KeyRefusal::Weak { path, weakness }) => {
                assert_eq!(path, r"HKCU\Software");
                assert!(
                    matches!(&weakness, Weakness::UntrustedOwner(owner) if *owner == sid)
                        || matches!(weakness, Weakness::Writable { .. }),
                    "{weakness:?}"
                );
            }
            other => panic!("expected a weak key, got {other:?}"),
        }
    }

    #[test]
    fn a_symbolic_link_on_the_path_is_refused_without_being_followed() {
        let key = TestKey::new();
        let sid = inspect_current().expect("token").token.user_sid;
        let target = format!(r"\Registry\User\{sid}\{}\target", key.subkey);
        // The target exists and is a plain key; `link` leads to it.
        drop(create(key.handle, "target", false).map(Key));
        make_link(&key, "link", &target);

        // Facts read off the link key itself: it is a link, and so a path through it is refused
        // at the link, before anything about its rights matters.
        let (_, link_facts) = open_and_read(key.handle, "link", RegistryView::Native)
            .expect("readable")
            .expect("exists");
        assert!(link_facts.is_link);
        let (_, plain_facts) = open_and_read(key.handle, "target", RegistryView::Native)
            .expect("readable")
            .expect("exists");
        assert!(!plain_facts.is_link);

        let through = walk(key.handle, "ROOT", r"link\below", RegistryView::Native);
        assert_eq!(
            through,
            Err(KeyRefusal::SymbolicLink {
                path: r"ROOT\link".to_owned()
            })
        );
        // The plain key is refused for another reason (the user owns it), not as a link.
        assert!(matches!(
            walk(key.handle, "ROOT", "target", RegistryView::Native),
            Err(KeyRefusal::Weak { .. })
        ));
    }

    #[test]
    fn a_missing_key_below_a_checked_parent_ends_the_walk_without_a_refusal() {
        let key = TestKey::new();
        // Nothing below the test key exists, and the walk below it never reaches a key to judge.
        assert_eq!(
            walk(key.handle, "ROOT", r"missing\deeper", RegistryView::Native),
            Ok(())
        );
    }
}
