//! The install-folder check: the helper (and the UI, before it asks for elevation) runs only from
//! a folder that no non-administrator can change.
//!
//! The elevated helper trusts its own image, the folder around it and its state folder. If a
//! standard user could write to any of them, "run the helper" would be "run what the user put
//! there with administrator rights". So, before it serves anything, the helper checks that
//!
//! - the image sits directly in `<Program Files>\Volt` (the Program Files known folder, not a path
//!   taken from the environment or the command line);
//! - the image, that folder and the state folder (when it exists) are owned by SYSTEM,
//!   Administrators or `TrustedInstaller`;
//! - no entry of their DACLs grants write, append, delete, create, change-permissions or
//!   take-ownership (or a generic right that includes them) to anyone but those three;
//! - none of them is a reparse point (junction or symbolic link).
//!
//! Ancestors above Program Files are not walked: the drive root legitimately lets Authenticated
//! Users create folders, and Program Files itself is protected by Windows. The real gate is the
//! ACL the installer leaves on the folder; this check is the tripwire that notices when it is
//! not there. It fails closed: anything it cannot read is a refusal.

use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};

use windows::Win32::UI::Shell::FOLDERID_ProgramFiles;

use super::acl::{self, Ace, INHERIT_ONLY_ACE, SecurityInfo};
use super::identity::normalise_path;
use crate::folders::shell_folder;

/// The folder under Program Files the product installs into.
pub const INSTALL_FOLDER_NAME: &str = "Volt";
/// The helper's state folder inside the install folder.
pub const STATE_FOLDER_NAME: &str = "state";

/// SID of SYSTEM.
const SID_SYSTEM: &str = "S-1-5-18";
/// SID of the Administrators group.
const SID_ADMINISTRATORS: &str = "S-1-5-32-544";
/// SID of the `TrustedInstaller` service.
const SID_TRUSTED_INSTALLER: &str =
    "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464";
/// SID of Everyone, reported for a NULL DACL.
const SID_EVERYONE: &str = "S-1-1-0";

const FILE_WRITE_DATA: u32 = 0x0000_0002; // also FILE_ADD_FILE
const FILE_APPEND_DATA: u32 = 0x0000_0004; // also FILE_ADD_SUBDIRECTORY
const FILE_WRITE_EA: u32 = 0x0000_0010;
const FILE_DELETE_CHILD: u32 = 0x0000_0040;
const FILE_WRITE_ATTRIBUTES: u32 = 0x0000_0100;
const DELETE: u32 = 0x0001_0000;
const WRITE_DAC: u32 = 0x0004_0000;
const WRITE_OWNER: u32 = 0x0008_0000;
const GENERIC_ALL: u32 = 0x1000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const MAXIMUM_ALLOWED: u32 = 0x0200_0000;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;

/// Every right that lets a principal change, replace, delete or re-permission an object.
pub const WRITE_RIGHTS: u32 = FILE_WRITE_DATA
    | FILE_APPEND_DATA
    | FILE_WRITE_EA
    | FILE_DELETE_CHILD
    | FILE_WRITE_ATTRIBUTES
    | DELETE
    | WRITE_DAC
    | WRITE_OWNER
    | GENERIC_ALL
    | GENERIC_WRITE
    | MAXIMUM_ALLOWED;

const KEY_SET_VALUE: u32 = 0x0000_0002;
const KEY_CREATE_SUB_KEY: u32 = 0x0000_0004;
const KEY_CREATE_LINK: u32 = 0x0000_0020;

/// Every right that lets a principal change, replace, delete or re-permission a registry key:
/// set a value, create a subkey or a link, and the standard and generic write rights. (The file
/// rights above share some bit positions with these but not all, so a key is judged by its own
/// list.)
pub const KEY_WRITE_RIGHTS: u32 = KEY_SET_VALUE
    | KEY_CREATE_SUB_KEY
    | KEY_CREATE_LINK
    | DELETE
    | WRITE_DAC
    | WRITE_OWNER
    | GENERIC_ALL
    | GENERIC_WRITE
    | MAXIMUM_ALLOWED;

/// Whether `sid` is one of the principals that may own and change the install folder.
#[must_use]
pub fn is_trusted_principal(sid: &str) -> bool {
    matches!(sid, SID_SYSTEM | SID_ADMINISTRATORS | SID_TRUSTED_INSTALLER)
}

/// What is wrong with an object's owner or DACL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Weakness {
    /// Owned by someone who is not SYSTEM, Administrators or `TrustedInstaller` (an owner can always
    /// rewrite the DACL).
    UntrustedOwner(String),
    /// A grant of write-like rights to someone who is not SYSTEM, Administrators or `TrustedInstaller`.
    Writable {
        /// Who.
        sid: String,
        /// The access mask of the entry.
        mask: u32,
    },
}

/// Checks one file or folder's security. Pure: it only looks at `info`.
///
/// Deny entries are ignored (a grant is judged on its own, which is the stricter reading), and so
/// are entries that apply to children only. A NULL DACL grants everyone everything.
///
/// # Errors
/// The first [`Weakness`] found.
pub fn check_security(info: &SecurityInfo) -> Result<(), Weakness> {
    check_security_for(info, WRITE_RIGHTS)
}

/// [`check_security`] for an object whose write rights are `write_rights` (a registry key has
/// its own, [`KEY_WRITE_RIGHTS`]).
///
/// # Errors
/// The first [`Weakness`] found.
pub fn check_security_for(info: &SecurityInfo, write_rights: u32) -> Result<(), Weakness> {
    if !is_trusted_principal(&info.owner) {
        return Err(Weakness::UntrustedOwner(info.owner.clone()));
    }
    let Some(aces) = &info.dacl else {
        return Err(Weakness::Writable {
            sid: SID_EVERYONE.to_owned(),
            mask: GENERIC_ALL,
        });
    };
    for Ace {
        allow,
        sid,
        mask,
        flags,
    } in aces
    {
        if *allow
            && flags & INHERIT_ONLY_ACE == 0
            && !is_trusted_principal(sid)
            && mask & write_rights != 0
        {
            return Err(Weakness::Writable {
                sid: sid.clone(),
                mask: *mask,
            });
        }
    }
    Ok(())
}

/// The three paths the check covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallLayout {
    /// The folder the image sits in.
    pub install_dir: PathBuf,
    /// The executable that is running (or about to start the helper).
    pub image: PathBuf,
    /// The helper's state folder (it may not exist yet).
    pub state_dir: PathBuf,
}

impl InstallLayout {
    /// The layout around `image`: its folder, and `state` inside it.
    #[must_use]
    pub fn around(image: &Path) -> Option<Self> {
        let install_dir = image.parent()?.to_path_buf();
        Some(Self {
            state_dir: install_dir.join(STATE_FOLDER_NAME),
            install_dir,
            image: image.to_path_buf(),
        })
    }

    /// The sibling file `name` in the install folder (the UI executable, for the helper).
    #[must_use]
    pub fn sibling(&self, name: &str) -> PathBuf {
        self.install_dir.join(name)
    }
}

/// Why the install folder is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallRefusal {
    /// The image is not directly in `<Program Files>\Volt`.
    NotInProgramFiles {
        /// Where it must be.
        expected: PathBuf,
        /// Where it is.
        actual: PathBuf,
    },
    /// The Program Files folder could not be resolved.
    ProgramFilesUnknown(String),
    /// A reparse point (junction or symbolic link) stands where the real file or folder must be.
    ReparsePoint(PathBuf),
    /// An owner or ACL that lets a non-administrator change the object.
    Weak {
        /// The object.
        path: PathBuf,
        /// What is wrong with it.
        weakness: Weakness,
    },
    /// The object could not be inspected. Not knowing is a refusal.
    Unreadable {
        /// The object.
        path: PathBuf,
        /// Why.
        reason: String,
    },
}

impl std::fmt::Display for InstallRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotInProgramFiles { expected, actual } => write!(
                f,
                "not installed in the Program Files folder (expected {}, found {})",
                expected.display(),
                actual.display()
            ),
            Self::ProgramFilesUnknown(reason) => {
                write!(f, "the Program Files folder is unknown ({reason})")
            }
            Self::ReparsePoint(path) => write!(f, "{} is a reparse point", path.display()),
            Self::Weak { path, weakness } => match weakness {
                Weakness::UntrustedOwner(owner) => {
                    write!(f, "{} is owned by {owner}", path.display())
                }
                Weakness::Writable { sid, .. } => {
                    write!(f, "{} can be changed by {sid}", path.display())
                }
            },
            Self::Unreadable { path, reason } => {
                write!(f, "{} cannot be checked ({reason})", path.display())
            }
        }
    }
}

impl std::error::Error for InstallRefusal {}

/// Checks the install of the running executable: resolves the Program Files known folder and
/// runs [`verify_layout`] on the layout around `image`.
///
/// # Errors
/// The first [`InstallRefusal`].
pub fn verify_install(image: &Path) -> Result<InstallLayout, InstallRefusal> {
    let program_files = shell_folder(&FOLDERID_ProgramFiles, "ProgramFiles")
        .map_err(|error| InstallRefusal::ProgramFilesUnknown(error.to_string()))?;
    let layout = InstallLayout::around(image).ok_or_else(|| InstallRefusal::Unreadable {
        path: image.to_path_buf(),
        reason: "the image has no folder".to_owned(),
    })?;
    verify_layout(&layout, &program_files)?;
    Ok(layout)
}

/// The whole check for a layout under a given Program Files folder: location, then reparse points
/// and security of the image, its folder and the state folder.
///
/// # Errors
/// The first [`InstallRefusal`].
pub fn verify_layout(layout: &InstallLayout, program_files: &Path) -> Result<(), InstallRefusal> {
    let expected = program_files.join(INSTALL_FOLDER_NAME);
    if normalise_path(&layout.install_dir.to_string_lossy())
        != normalise_path(&expected.to_string_lossy())
    {
        return Err(InstallRefusal::NotInProgramFiles {
            expected,
            actual: layout.install_dir.clone(),
        });
    }
    verify_tree(layout)
}

/// Reparse points and security of the image, the folder and the state folder (when it exists).
///
/// # Errors
/// The first [`InstallRefusal`].
pub fn verify_tree(layout: &InstallLayout) -> Result<(), InstallRefusal> {
    // The folder first: if it is a junction, nothing inside it means what it says.
    for (path, required) in [
        (&layout.install_dir, true),
        (&layout.image, true),
        (&layout.state_dir, false),
    ] {
        match std::fs::symlink_metadata(path) {
            Ok(metadata) => {
                if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                    return Err(InstallRefusal::ReparsePoint(path.clone()));
                }
            }
            Err(error) if !required && error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(InstallRefusal::Unreadable {
                    path: path.clone(),
                    reason: error.to_string(),
                });
            }
        }
        let info = acl::of_path(path).map_err(|reason| InstallRefusal::Unreadable {
            path: path.clone(),
            reason,
        })?;
        check_security(&info).map_err(|weakness| InstallRefusal::Weak {
            path: path.clone(),
            weakness,
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const USER: &str = "S-1-5-21-111-222-333-1001";

    fn ace(allow: bool, sid: &str, mask: u32, flags: u8) -> Ace {
        Ace {
            allow,
            sid: sid.to_owned(),
            mask,
            flags,
        }
    }

    fn info(owner: &str, aces: Vec<Ace>) -> SecurityInfo {
        SecurityInfo {
            owner: owner.to_owned(),
            dacl: Some(aces),
        }
    }

    const READ_EXECUTE: u32 = 0x0012_00a9;
    const MODIFY: u32 = 0x0013_01bf;
    const FULL: u32 = 0x001f_01ff;

    /// What a stock `Program Files\Volt` looks like: owned by Administrators, full control for the
    /// three trusted principals, read and execute for Users and application packages.
    fn stock() -> SecurityInfo {
        info(
            SID_ADMINISTRATORS,
            vec![
                ace(true, SID_TRUSTED_INSTALLER, FULL, 0),
                ace(true, SID_SYSTEM, MODIFY, 0),
                ace(true, SID_ADMINISTRATORS, MODIFY, 0),
                ace(true, "S-1-5-32-545", READ_EXECUTE, 0),
                ace(true, "S-1-15-2-1", READ_EXECUTE, 0),
                // CREATOR OWNER full control, but for children only.
                ace(true, "S-1-3-0", FULL, INHERIT_ONLY_ACE | 0x03),
            ],
        )
    }

    #[test]
    fn a_stock_program_files_acl_passes() {
        assert_eq!(check_security(&stock()), Ok(()));
    }

    #[test]
    fn each_write_like_right_for_a_user_is_refused() {
        for (name, mask) in [
            ("write data / add file", FILE_WRITE_DATA),
            ("append data / add subfolder", FILE_APPEND_DATA),
            ("write extended attributes", FILE_WRITE_EA),
            ("delete child", FILE_DELETE_CHILD),
            ("write attributes", FILE_WRITE_ATTRIBUTES),
            ("delete", DELETE),
            ("change permissions", WRITE_DAC),
            ("take ownership", WRITE_OWNER),
            ("generic all", GENERIC_ALL),
            ("generic write", GENERIC_WRITE),
            ("maximum allowed", MAXIMUM_ALLOWED),
            ("modify", MODIFY),
            ("full control", FULL),
        ] {
            let mut weak = stock();
            if let Some(aces) = &mut weak.dacl {
                aces.push(ace(true, USER, mask | READ_EXECUTE, 0));
            }
            assert_eq!(
                check_security(&weak),
                Err(Weakness::Writable {
                    sid: USER.to_owned(),
                    mask: mask | READ_EXECUTE
                }),
                "{name}"
            );
        }
    }

    #[test]
    fn well_known_groups_count_like_any_other_user() {
        for sid in ["S-1-1-0", "S-1-5-11", "S-1-5-32-545", "S-1-3-0"] {
            let mut weak = stock();
            if let Some(aces) = &mut weak.dacl {
                aces.push(ace(true, sid, FILE_WRITE_DATA, 0));
            }
            assert!(
                matches!(check_security(&weak), Err(Weakness::Writable { .. })),
                "{sid}"
            );
        }
    }

    #[test]
    fn read_only_access_for_a_user_is_fine() {
        let mut ok = stock();
        if let Some(aces) = &mut ok.dacl {
            aces.push(ace(true, USER, READ_EXECUTE, 0));
        }
        assert_eq!(check_security(&ok), Ok(()));
    }

    #[test]
    fn an_entry_for_children_only_does_not_count_but_a_plain_one_does() {
        let mut inherit_only = stock();
        if let Some(aces) = &mut inherit_only.dacl {
            aces.push(ace(true, USER, FULL, INHERIT_ONLY_ACE | 0x03));
        }
        assert_eq!(check_security(&inherit_only), Ok(()));

        let mut inherited = stock();
        if let Some(aces) = &mut inherited.dacl {
            // Inherited from a parent (0x10) and applying to this object.
            aces.push(ace(true, USER, FILE_WRITE_DATA, 0x10));
        }
        assert!(check_security(&inherited).is_err());
    }

    #[test]
    fn a_deny_entry_does_not_hide_a_grant() {
        let mut weak = stock();
        if let Some(aces) = &mut weak.dacl {
            aces.push(ace(false, USER, FILE_WRITE_DATA, 0));
            aces.push(ace(true, USER, FILE_WRITE_DATA, 0));
        }
        assert!(check_security(&weak).is_err());
    }

    #[test]
    fn a_user_owner_is_refused_even_with_a_clean_acl() {
        let mut owned = stock();
        owned.owner = USER.to_owned();
        assert_eq!(
            check_security(&owned),
            Err(Weakness::UntrustedOwner(USER.to_owned()))
        );
        for trusted in [SID_SYSTEM, SID_ADMINISTRATORS, SID_TRUSTED_INSTALLER] {
            let mut ok = stock();
            ok.owner = trusted.to_owned();
            assert_eq!(check_security(&ok), Ok(()), "{trusted}");
        }
    }

    #[test]
    fn a_null_dacl_is_refused() {
        let null = SecurityInfo {
            owner: SID_ADMINISTRATORS.to_owned(),
            dacl: None,
        };
        assert!(matches!(
            check_security(&null),
            Err(Weakness::Writable { .. })
        ));
    }

    fn layout_in(root: &Path) -> InstallLayout {
        let install_dir = root.join(INSTALL_FOLDER_NAME);
        std::fs::create_dir_all(&install_dir).expect("folder");
        let image = install_dir.join("volt-helper.exe");
        std::fs::write(&image, b"not a program").expect("image");
        InstallLayout::around(&image).expect("layout")
    }

    #[test]
    fn an_image_outside_program_files_is_refused() {
        let temp = tempfile::tempdir().expect("temp");
        let layout = layout_in(temp.path());
        // The "Program Files" this check is given is some other folder.
        let elsewhere = temp.path().join("elsewhere");
        assert!(matches!(
            verify_layout(&layout, &elsewhere),
            Err(InstallRefusal::NotInProgramFiles { .. })
        ));
        // Deeper than Program Files\Volt is not the install folder either.
        let nested = temp.path().join(INSTALL_FOLDER_NAME).join("bin");
        std::fs::create_dir_all(&nested).expect("nested");
        let deeper = InstallLayout::around(&nested.join("volt-helper.exe")).expect("layout");
        assert!(matches!(
            verify_layout(&deeper, temp.path()),
            Err(InstallRefusal::NotInProgramFiles { .. })
        ));
        // A look-alike folder name is not it.
        let lookalike = temp.path().join("Volt2");
        std::fs::create_dir_all(&lookalike).expect("lookalike");
        let other = InstallLayout::around(&lookalike.join("volt-helper.exe")).expect("layout");
        assert!(matches!(
            verify_layout(&other, temp.path()),
            Err(InstallRefusal::NotInProgramFiles { .. })
        ));
    }

    #[test]
    fn the_real_acl_of_a_folder_the_user_made_is_refused() {
        // A temp folder is owned by, and writable for, the user: exactly what the helper must not
        // run from. This reads the real ACLs through the same code the helper uses.
        let temp = tempfile::tempdir().expect("temp");
        let layout = layout_in(temp.path());
        match verify_layout(&layout, temp.path()) {
            Err(InstallRefusal::Weak { path, weakness }) => {
                assert_eq!(path, layout.install_dir, "the folder is checked first");
                assert!(matches!(
                    weakness,
                    Weakness::UntrustedOwner(_) | Weakness::Writable { .. }
                ));
            }
            other => panic!("expected a weak folder, got {other:?}"),
        }
    }

    #[test]
    fn a_junction_where_the_folder_must_be_is_refused() {
        let temp = tempfile::tempdir().expect("temp");
        let target = temp.path().join("real");
        std::fs::create_dir_all(&target).expect("target");
        std::fs::write(target.join("volt-helper.exe"), b"x").expect("image");
        let link = temp.path().join(INSTALL_FOLDER_NAME);
        let made = std::process::Command::new("cmd")
            .arg("/C")
            .arg("mklink")
            .arg("/J")
            .arg(&link)
            .arg(&target)
            .stdout(std::process::Stdio::null())
            .status();
        if !made.is_ok_and(|status| status.success()) {
            // A machine that cannot make junctions cannot run this case.
            return;
        }
        let layout = InstallLayout::around(&link.join("volt-helper.exe")).expect("layout");
        assert_eq!(
            verify_layout(&layout, temp.path()),
            Err(InstallRefusal::ReparsePoint(link.clone()))
        );
        // Remove the junction itself (not its target) before the temp folder goes.
        let _ = std::fs::remove_dir(&link);
    }

    #[test]
    fn a_missing_image_or_folder_is_unreadable_but_a_missing_state_folder_is_fine() {
        let temp = tempfile::tempdir().expect("temp");
        let layout = InstallLayout::around(&temp.path().join("Volt").join("x.exe")).expect("l");
        assert!(matches!(
            verify_tree(&layout),
            Err(InstallRefusal::Unreadable { .. })
        ));
    }

    #[test]
    fn a_system_folder_passes_and_the_state_folder_may_be_missing() {
        // The Windows system folder is owned by `TrustedInstaller` and writable only by the trusted
        // principals, so the real reading code and the rules agree it is sound.
        let windir = std::env::var_os("SystemRoot").map(PathBuf::from);
        let Some(system32) = windir.map(|dir| dir.join("System32")) else {
            return;
        };
        let image = system32.join("kernel32.dll");
        let Some(mut layout) = InstallLayout::around(&image) else {
            return;
        };
        layout.state_dir = system32.join("volt-no-such-state-folder");
        assert_eq!(verify_tree(&layout), Ok(()));
    }

    #[test]
    fn the_known_folder_check_runs_on_this_machine() {
        // The real image of this test binary is in a build folder, not in Program Files: refused,
        // and with the right reason. Nothing is started or changed.
        let exe = std::env::current_exe().expect("exe");
        assert!(matches!(
            verify_install(&exe),
            Err(InstallRefusal::NotInProgramFiles { .. })
        ));
    }
}
