//! [`FileSystem`] port on the Windows shell and file APIs.

use std::fs;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use volt_core::{DirEntry, EntryInfo, EntryKind, FileStat, FileSystem, PortError, Timestamp};
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
const FILE_ATTRIBUTE_SYSTEM: u32 = 0x4;
const ERROR_ACCESS_DENIED: u32 = 5;
const ERROR_SHARING_VIOLATION: u32 = 32;
const ERROR_LOCK_VIOLATION: u32 = 33;

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

/// What a directory entry is, from metadata that does not follow links.
fn entry_info(meta: &fs::Metadata) -> EntryInfo {
    let kind = if is_reparse_point(meta) {
        EntryKind::ReparsePoint
    } else if meta.is_dir() {
        EntryKind::Directory
    } else {
        EntryKind::File
    };
    EntryInfo {
        kind,
        size_bytes: if kind == EntryKind::File {
            meta.len()
        } else {
            0
        },
        modified: meta.modified().ok().map(timestamp_of),
        system: meta.file_attributes() & FILE_ATTRIBUTE_SYSTEM != 0,
    }
}

fn timestamp_of(time: SystemTime) -> Timestamp {
    let millis = match time.duration_since(UNIX_EPOCH) {
        Ok(after) => i64::try_from(after.as_millis()).unwrap_or(i64::MAX),
        Err(before) => i64::try_from(before.duration().as_millis()).map_or(i64::MIN, |m| -m),
    };
    Timestamp::from_unix_millis(millis)
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

    fn inspect(&self, path: &Path) -> Result<Option<EntryInfo>, PortError> {
        // symlink_metadata: describe the entry itself, never its target.
        match fs::symlink_metadata(path) {
            Ok(meta) => Ok(Some(entry_info(&meta))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(from_io(&e, &path.display().to_string())),
        }
    }

    fn read_dir(&self, dir: &Path) -> Result<Vec<DirEntry>, PortError> {
        let context = dir.display().to_string();
        let meta = fs::symlink_metadata(dir).map_err(|e| from_io(&e, &context))?;
        if is_reparse_point(&meta) || !meta.is_dir() {
            return Err(PortError::InvalidInput(format!(
                "{context} is not a plain directory (a link or other reparse point is never listed)"
            )));
        }
        let mut entries = Vec::new();
        for entry in fs::read_dir(dir).map_err(|e| from_io(&e, &context))? {
            // A failure while walking the listing fails the listing: a partial
            // directory must not look like a complete one.
            let entry = entry.map_err(|e| from_io(&e, &context))?;
            // `DirEntry::metadata` does not traverse symbolic links on
            // Windows and reuses the data of the directory listing, so it costs
            // no extra open per file.
            let info = entry
                .metadata()
                .map(|meta| entry_info(&meta))
                .map_err(|e| from_io(&e, &entry.path().display().to_string()));
            entries.push(DirEntry {
                name: entry.file_name(),
                info,
            });
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(entries)
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

/// The error for a Recycle Bin operation the shell aborted. The shell does not
/// say why (with its UI suppressed a locked file just aborts the operation), so
/// a file is probed: when it cannot be opened exclusively because another
/// program holds it, or access is denied, the error says so
/// ([`PortError::AccessDenied`]) and a cleaner can report it as locked.
fn aborted(path: &Path, context: &str) -> PortError {
    if path.is_file()
        && let Err(error) = fs::OpenOptions::new().read(true).share_mode(0).open(path)
        && matches!(
            error.raw_os_error().map(i32::cast_unsigned),
            Some(ERROR_ACCESS_DENIED | ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION)
        )
    {
        return from_io(&error, context);
    }
    PortError::Other(format!("{context}: the Recycle Bin operation was aborted"))
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
            return Err(aborted(path, &context));
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

    // ---- inspect and read_dir ------------------------------------------------------------

    fn set_age(path: &Path, days: u64) {
        let file = fs::OpenOptions::new().write(true).open(path).expect("open");
        file.set_modified(SystemTime::now() - std::time::Duration::from_secs(days * 86_400))
            .expect("set mtime");
    }

    #[test]
    fn inspect_reports_kind_size_and_modification_time_without_following_links() {
        let dir = TempDir::new("inspect");
        let file = dir.0.join("a.bin");
        fs::write(&file, [0_u8; 321]).expect("write");
        set_age(&file, 10);
        let folder = dir.0.join("folder");
        fs::create_dir(&folder).expect("mkdir");
        let link = dir.0.join("link");
        make_junction(&link, &folder);
        let fsys = WinFileSystem::new();

        let info = fsys.inspect(&file).expect("inspect").expect("exists");
        assert_eq!((info.kind, info.size_bytes), (EntryKind::File, 321));
        let age_ms = timestamp_of(SystemTime::now()).as_unix_millis()
            - info.modified.expect("mtime").as_unix_millis();
        assert!(
            (9 * 86_400_000..11 * 86_400_000).contains(&age_ms),
            "age {age_ms} ms"
        );
        assert_eq!(
            fsys.inspect(&folder).expect("inspect").map(|i| i.kind),
            Some(EntryKind::Directory)
        );
        // The junction is described as itself.
        assert_eq!(
            fsys.inspect(&link)
                .expect("inspect")
                .map(|i| (i.kind, i.size_bytes)),
            Some((EntryKind::ReparsePoint, 0))
        );
        assert_eq!(fsys.inspect(&dir.0.join("missing")), Ok(None));
    }

    #[test]
    fn read_dir_lists_sorted_entries_and_refuses_links_and_files() {
        let dir = TempDir::new("readdir");
        fs::write(dir.0.join("b.txt"), "b").expect("write");
        fs::write(dir.0.join("a.txt"), "aa").expect("write");
        fs::create_dir(dir.0.join("sub")).expect("mkdir");
        let target = dir.0.join("sub");
        let link = dir.0.join("link");
        make_junction(&link, &target);
        let fsys = WinFileSystem::new();

        let listing = fsys.read_dir(&dir.0).expect("list");
        let summary: Vec<_> = listing
            .iter()
            .map(|e| {
                (
                    e.name.to_string_lossy().into_owned(),
                    e.info.as_ref().expect("info").kind,
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                ("a.txt".to_owned(), EntryKind::File),
                ("b.txt".to_owned(), EntryKind::File),
                ("link".to_owned(), EntryKind::ReparsePoint),
                ("sub".to_owned(), EntryKind::Directory),
            ]
        );
        assert_eq!(listing[0].info.as_ref().expect("info").size_bytes, 2);
        // Listing the junction would walk into its target: refused.
        assert!(matches!(
            fsys.read_dir(&link),
            Err(PortError::InvalidInput(_))
        ));
        assert!(matches!(
            fsys.read_dir(&dir.0.join("a.txt")),
            Err(PortError::InvalidInput(_))
        ));
        assert!(matches!(
            fsys.read_dir(&dir.0.join("missing")),
            Err(PortError::NotFound(_))
        ));
    }

    // ---- the cleaning engine over the real file system -----------------------------------------

    /// Resolves the temp location to a test directory and protects nothing.
    struct TestFolders(PathBuf);

    impl volt_core::KnownFolders for TestFolders {
        fn location(&self, location: volt_core::Location) -> Result<PathBuf, PortError> {
            match location {
                volt_core::Location::UserTemp => Ok(self.0.clone()),
                other => Err(PortError::NotFound(format!("{other}"))),
            }
        }

        fn protected_folders(&self) -> Result<Vec<PathBuf>, PortError> {
            Ok(Vec::new())
        }
    }

    fn engine_for(root: &Path, min_age_days: u32) -> volt_core::CleanEngine {
        let rules = volt_core::CleaningRules::from_toml(&format!(
            r#"
[[rule]]
id = "test-rule"
title = "Test"
description = "Test rule."
category = "temporary_files"
risk = "safe"
include = ["**"]
root = {{ location = "user_temp" }}
constraints = {{ min_age_days = {min_age_days}, max_depth = 5, remove_empty_dirs = true }}
"#
        ))
        .expect("rules");
        volt_core::CleanEngine::new(
            rules,
            std::sync::Arc::new(TestFolders(root.to_path_buf())),
            std::sync::Arc::new(WinFileSystem::new()),
            std::sync::Arc::new(volt_core::SystemClock),
        )
    }

    fn planned_names(scan: &volt_core::RuleScan, root: &Path) -> Vec<String> {
        scan.items
            .iter()
            .map(|i| {
                i.path
                    .strip_prefix(root)
                    .expect("inside root")
                    .to_string_lossy()
                    .into_owned()
            })
            .collect()
    }

    #[test]
    fn a_real_scan_reports_sizes_and_ages_and_never_follows_a_junction() {
        let dir = TempDir::new("scan");
        let root = dir.0.join("root");
        let outside = dir.0.join("outside");
        fs::create_dir_all(root.join("sub")).expect("mkdir");
        fs::create_dir_all(&outside).expect("mkdir");
        let old = root.join("old.tmp");
        fs::write(&old, [1_u8; 100]).expect("write");
        set_age(&old, 5);
        fs::write(root.join("new.tmp"), "fresh").expect("write");
        let nested = root.join("sub").join("nested.tmp");
        fs::write(&nested, [2_u8; 28]).expect("write");
        set_age(&nested, 5);
        // Precious data outside the root, reachable through two junctions.
        let victim = outside.join("victim.docx");
        fs::write(&victim, "keep").expect("write");
        set_age(&victim, 400);
        make_junction(&root.join("escape"), &outside);
        make_junction(&root.join("sub").join("escape2"), &outside);

        let engine = engine_for(&root, 2);
        let plan = engine.scan_all();
        let scan = &plan.rules[0];

        assert_eq!(scan.failure, None);
        assert_eq!(
            planned_names(scan, &root),
            ["old.tmp", "sub\\nested.tmp"],
            "{scan:#?}"
        );
        assert_eq!(scan.total_bytes(), 128);
        assert_eq!(scan.counts.links, 2);
        assert_eq!(scan.counts.too_new, 1);
        assert!(
            scan.items.iter().all(|i| i.path.starts_with(&root)),
            "nothing outside the root may be planned"
        );
        assert!(victim.exists());
        // Nothing was changed by scanning.
        assert!(old.exists() && nested.exists());
        for link in [root.join("escape"), root.join("sub").join("escape2")] {
            fs::remove_dir(&link).expect("remove junction");
        }
    }

    #[test]
    fn a_junction_swapped_in_after_the_scan_is_not_followed_by_the_clean() {
        let dir = TempDir::new("swap");
        let root = dir.0.join("root");
        let outside = dir.0.join("outside");
        fs::create_dir_all(root.join("sub")).expect("mkdir");
        fs::create_dir_all(&outside).expect("mkdir");
        fs::write(root.join("sub").join("a.tmp"), "x").expect("write");
        fs::write(outside.join("a.tmp"), "precious").expect("write");

        let engine = engine_for(&root, 0);
        let plan = engine.scan_all();
        assert_eq!(plan.rules[0].file_count(), 1);

        // Replace `sub` with a junction to `outside`, which holds a file of
        // the same name.
        fs::remove_dir_all(root.join("sub")).expect("remove sub");
        make_junction(&root.join("sub"), &outside);

        let report = engine.clean(&plan.confirm(), volt_core::Source::Manual);

        assert_eq!(report.moved_files(), 0);
        assert!(
            report.rules[0]
                .skipped
                .iter()
                .all(|s| s.reason == volt_core::CleanSkipReason::Link),
            "{report:#?}"
        );
        assert_eq!(
            fs::read_to_string(outside.join("a.tmp")).expect("still there"),
            "precious"
        );
        fs::remove_dir(root.join("sub")).expect("remove junction");
    }

    #[test]
    fn a_file_that_changed_after_the_scan_is_left_alone() {
        let dir = TempDir::new("changed");
        let root = dir.0.join("root");
        fs::create_dir_all(&root).expect("mkdir");
        let file = root.join("a.tmp");
        fs::write(&file, "one").expect("write");

        let engine = engine_for(&root, 0);
        let plan = engine.scan_all();
        fs::write(&file, "now longer").expect("rewrite");

        let report = engine.clean(&plan.confirm(), volt_core::Source::Manual);
        assert_eq!(report.moved_files(), 0);
        assert_eq!(
            report.rules[0].skipped[0].reason,
            volt_core::CleanSkipReason::Changed
        );
        assert_eq!(fs::read_to_string(&file).expect("kept"), "now longer");
    }

    /// Leaves `volt-test-*` items in the user's Recycle Bin, so it only runs on
    /// request: `cargo test -p volt-win -- --ignored recycle_bin`.
    #[test]
    #[ignore = "leaves volt-test-* items in the real Recycle Bin"]
    fn recycle_bin_receives_a_cleaned_plan_and_a_locked_file_is_skipped() {
        let dir = TempDir::new("recycle-clean");
        let root = dir.0.join("root");
        fs::create_dir_all(root.join("emptied")).expect("mkdir");
        fs::write(root.join("emptied").join("inner.tmp"), [0_u8; 64]).expect("write");
        fs::write(root.join("top.tmp"), [0_u8; 36]).expect("write");
        let locked = root.join("locked.tmp");
        fs::write(&locked, "held").expect("write");

        let engine = engine_for(&root, 0);
        let plan = engine.scan_all();
        assert_eq!(plan.rules[0].file_count(), 3);
        // Another program holds one file open without sharing.
        let _held = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&locked)
            .expect("open exclusively");

        let report = engine.clean(&plan.confirm(), volt_core::Source::Manual);

        assert_eq!(report.moved_files(), 2, "{report:#?}");
        assert_eq!(report.moved_directories(), 1);
        assert_eq!(report.moved_bytes(), 100);
        assert_eq!(report.rules[0].skipped.len(), 1);
        assert_eq!(
            report.rules[0].skipped[0].reason,
            volt_core::CleanSkipReason::LockedOrDenied
        );
        assert!(locked.exists());
        assert!(!root.join("top.tmp").exists());
        assert!(!root.join("emptied").exists());
    }
}
