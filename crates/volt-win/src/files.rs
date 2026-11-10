//! [`FileSystem`] port on the Windows shell and file APIs.

use std::fs;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

use volt_core::{FileStat, FileSystem, PortError};
use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::Win32::UI::Shell::{
    FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FOFX_RECYCLEONDELETE,
    FileOperation, IFileOperation, IShellItem, SHCreateItemFromParsingName,
};
use windows::core::PCWSTR;

use crate::error::{from_hresult, from_io};

const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

/// The file system port backed by the Windows file APIs and the shell.
#[derive(Debug, Default, Clone, Copy)]
pub struct WinFileSystem;

impl WinFileSystem {
    /// A file system adapter.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

fn is_reparse_point(meta: &fs::Metadata) -> bool {
    meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

impl FileSystem for WinFileSystem {
    fn stat(&self, path: &Path) -> Result<Option<FileStat>, PortError> {
        // symlink_metadata: describe the entry itself, never its target.
        match fs::symlink_metadata(path) {
            Ok(meta) => Ok(Some(FileStat {
                size_bytes: if meta.is_dir() { 0 } else { meta.len() },
                is_dir: meta.is_dir(),
            })),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(from_io(&e, &path.display().to_string())),
        }
    }

    fn move_to_trash(&self, root: &Path, path: &Path) -> Result<(), PortError> {
        let checked = check_inside_root(root, path)?;
        recycle(&checked)
    }
}

/// Verifies that `path` is strictly inside `root` and reaches its target
/// without crossing a reparse point (junction, symlink, mount point).
///
/// The check is lexical first (no `..`, `path` starts with `root`), then walks
/// every component below the root with `symlink_metadata`, then confirms with
/// the fully resolved paths. Returns the path to hand to the shell.
///
/// This is a check-then-act sequence: a hostile process that swaps a directory
/// for a junction between the check and the delete can still win the race. The
/// guard defends against links that already exist, which is the cleaner's
/// threat model.
fn check_inside_root(root: &Path, path: &Path) -> Result<PathBuf, PortError> {
    let describe = |what: &str| {
        PortError::InvalidInput(format!("{} {what} {}", path.display(), root.display()))
    };
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(PortError::InvalidInput(format!(
            "{} contains a parent directory component",
            path.display()
        )));
    }
    let relative = path
        .strip_prefix(root)
        .map_err(|_| describe("is outside"))?;
    if relative.as_os_str().is_empty() {
        return Err(describe("is the root itself:"));
    }

    // Walk from the root down; every component below it must be a plain
    // file or directory.
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(meta) if is_reparse_point(&meta) => {
                return Err(PortError::InvalidInput(format!(
                    "{} is a junction or symbolic link; refusing to follow it out of {}",
                    current.display(),
                    root.display()
                )));
            }
            Ok(_) => {}
            Err(e) => return Err(from_io(&e, &path.display().to_string())),
        }
    }

    // Defence in depth: the resolved locations must agree.
    let resolved_root =
        fs::canonicalize(root).map_err(|e| from_io(&e, &root.display().to_string()))?;
    let resolved = fs::canonicalize(path).map_err(|e| from_io(&e, &path.display().to_string()))?;
    if !resolved.starts_with(&resolved_root) || resolved == resolved_root {
        return Err(describe("resolves outside"));
    }
    Ok(path.to_path_buf())
}

/// Initialises COM for the current thread and balances it on drop.
struct ComGuard {
    uninitialize: bool,
}

impl ComGuard {
    fn new() -> Result<Self, PortError> {
        // SAFETY: a null reserved pointer is required; the call only touches
        // the calling thread's COM state.
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        if hr.is_ok() {
            Ok(Self { uninitialize: true })
        } else if hr == RPC_E_CHANGED_MODE {
            // The thread already runs COM in another apartment model; that is
            // usable for IFileOperation and must not be uninitialised by us.
            Ok(Self {
                uninitialize: false,
            })
        } else {
            Err(from_hresult(hr.0, "initialise COM"))
        }
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.uninitialize {
            // SAFETY: balances the successful CoInitializeEx in `new` on the
            // same thread (the guard is neither Send nor moved across
            // threads, as it lives on one stack frame).
            unsafe { CoUninitialize() };
        }
    }
}

/// Moves one path to the Recycle Bin. Fails instead of deleting permanently
/// when the item cannot be recycled (`FOFX_RECYCLEONDELETE`).
fn recycle(path: &Path) -> Result<(), PortError> {
    let context = path.display().to_string();
    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    if wide.contains(&0) {
        return Err(PortError::InvalidInput(format!("{context} contains NUL")));
    }
    wide.push(0);

    let _com = ComGuard::new()?;

    // SAFETY: COM is initialised on this thread by `_com`; the CLSID is the
    // documented coclass of IFileOperation and no outer unknown is used.
    let operation: IFileOperation =
        unsafe { CoCreateInstance(&FileOperation, None, CLSCTX_ALL) }
            .map_err(|e| from_hresult(e.code().0, "create the shell file operation"))?;

    // SAFETY: `operation` is a valid IFileOperation; the flags are plain
    // values. UI is suppressed and recycling is mandatory.
    unsafe {
        operation.SetOperationFlags(
            FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT | FOFX_RECYCLEONDELETE,
        )
    }
    .map_err(|e| from_hresult(e.code().0, &context))?;

    // SAFETY: `wide` is a NUL-terminated UTF-16 path that outlives the call.
    let item: IShellItem = unsafe { SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None) }
        .map_err(|e| from_hresult(e.code().0, &context))?;

    // SAFETY: `operation` and `item` are valid COM interfaces owned by this
    // frame; queuing and performing the operation have no other preconditions.
    unsafe {
        operation
            .DeleteItem(&item, None)
            .map_err(|e| from_hresult(e.code().0, &context))?;
        operation
            .PerformOperations()
            .map_err(|e| from_hresult(e.code().0, &context))?;
        if operation
            .GetAnyOperationsAborted()
            .map_err(|e| from_hresult(e.code().0, &context))?
            .as_bool()
        {
            return Err(PortError::Other(format!(
                "{context}: the Recycle Bin operation was aborted"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::process::Command;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    /// A unique temp directory, removed on drop (also when a test fails).
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos());
            let dir = std::env::temp_dir().join(format!(
                "volt-test-{tag}-{}-{nanos}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&dir).expect("create temp dir");
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            // Junctions inside are removed first so the recursive delete can
            // never follow one into real data.
            if let Ok(entries) = fs::read_dir(&self.0) {
                for entry in entries.flatten() {
                    if entry.metadata().is_ok_and(|m| is_reparse_point(&m)) {
                        let _ = fs::remove_dir(entry.path());
                    }
                }
            }
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn make_junction(link: &Path, target: &Path) {
        let status = Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .expect("run mklink");
        assert!(
            status.status.success(),
            "mklink failed: {}",
            String::from_utf8_lossy(&status.stdout)
        );
    }

    #[test]
    fn stat_reports_files_dirs_and_missing_paths() {
        let dir = TempDir::new("stat");
        let file = dir.0.join("a.bin");
        fs::write(&file, [0_u8; 123]).expect("write");
        let fsys = WinFileSystem::new();
        assert_eq!(
            fsys.stat(&file),
            Ok(Some(FileStat {
                size_bytes: 123,
                is_dir: false
            }))
        );
        assert_eq!(
            fsys.stat(&dir.0),
            Ok(Some(FileStat {
                size_bytes: 0,
                is_dir: true
            }))
        );
        assert_eq!(fsys.stat(&dir.0.join("missing")), Ok(None));
        assert_eq!(fsys.stat(&dir.0.join("missing\\deeper")), Ok(None));
    }

    #[test]
    fn stat_does_not_follow_a_junction() {
        let dir = TempDir::new("statj");
        let target = dir.0.join("target");
        fs::create_dir(&target).expect("mkdir");
        fs::write(target.join("big"), [0_u8; 10]).expect("write");
        let link = dir.0.join("link");
        make_junction(&link, &target);
        // The junction is described as itself (a link, not a directory, with
        // no size), never as the directory it points to.
        assert_eq!(
            WinFileSystem::new().stat(&link),
            Ok(Some(FileStat {
                size_bytes: 0,
                is_dir: false
            }))
        );
    }

    #[test]
    fn trash_refuses_paths_outside_the_root() {
        let dir = TempDir::new("outside");
        let root = dir.0.join("root");
        fs::create_dir(&root).expect("mkdir");
        let outside = dir.0.join("outside.txt");
        fs::write(&outside, "keep").expect("write");
        let fsys = WinFileSystem::new();

        assert!(matches!(
            fsys.move_to_trash(&root, &outside),
            Err(PortError::InvalidInput(_))
        ));
        // Lexical escape through "..".
        let sneaky = root.join("..\\outside.txt");
        assert!(matches!(
            fsys.move_to_trash(&root, &sneaky),
            Err(PortError::InvalidInput(_))
        ));
        // Deleting the root itself is refused.
        assert!(matches!(
            fsys.move_to_trash(&root, &root),
            Err(PortError::InvalidInput(_))
        ));
        assert!(outside.exists(), "the file outside the root must survive");
    }

    #[test]
    fn trash_refuses_to_follow_a_junction_out_of_the_root() {
        let dir = TempDir::new("junction");
        let root = dir.0.join("root");
        let outside = dir.0.join("outside");
        fs::create_dir_all(&root).expect("mkdir");
        fs::create_dir_all(&outside).expect("mkdir");
        let victim = outside.join("victim.txt");
        fs::write(&victim, "keep").expect("write");
        let link = root.join("link");
        make_junction(&link, &outside);

        let fsys = WinFileSystem::new();
        // Through the junction.
        assert!(matches!(
            fsys.move_to_trash(&root, &link.join("victim.txt")),
            Err(PortError::InvalidInput(_))
        ));
        // The junction itself.
        assert!(matches!(
            fsys.move_to_trash(&root, &link),
            Err(PortError::InvalidInput(_))
        ));
        assert!(victim.exists(), "target behind the junction must survive");
        assert!(link.exists(), "the junction must be left alone");
    }

    #[test]
    fn trash_of_a_missing_path_is_not_found() {
        let dir = TempDir::new("missing");
        assert!(matches!(
            WinFileSystem::new().move_to_trash(&dir.0, &dir.0.join("nope.txt")),
            Err(PortError::NotFound(_))
        ));
    }

    /// Leaves `volt-test-*` items in the user's Recycle Bin, so it only runs on
    /// request: `cargo test -p volt-win -- --ignored recycle_bin`.
    #[test]
    #[ignore = "leaves a volt-test-* file in the real Recycle Bin"]
    fn recycle_bin_receives_files_and_directories() {
        let dir = TempDir::new("recycle");
        let file = dir.0.join("volt-test-file.txt");
        fs::write(&file, "recycle me").expect("write");
        let folder = dir.0.join("volt-test-folder");
        fs::create_dir(&folder).expect("mkdir");
        fs::write(folder.join("inner.txt"), "x").expect("write");

        let fsys = WinFileSystem::new();
        fsys.move_to_trash(&dir.0, &file).expect("trash file");
        fsys.move_to_trash(&dir.0, &folder).expect("trash folder");
        assert_eq!(fsys.stat(&file), Ok(None));
        assert_eq!(fsys.stat(&folder), Ok(None));
    }
}
