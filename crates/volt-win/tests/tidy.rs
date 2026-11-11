//! The tidy-up finder against the real file system adapter, over a temp directory. Scan only:
//! moving items would put them in the real Recycle Bin, which tests do not do.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use tempfile::TempDir;
use volt_core::{
    FileSystem as _, KnownFolders, Location, PortError, SystemClock, TidyFinder, TidyObserver,
    TidyOptions, TidyProgress,
};
use volt_win::WinFileSystem;

struct Folders;

impl KnownFolders for Folders {
    fn location(&self, location: Location) -> Result<PathBuf, PortError> {
        Err(PortError::NotFound(format!("{location}")))
    }

    fn protected_folders(&self) -> Result<Vec<PathBuf>, PortError> {
        Ok(Vec::new())
    }

    fn system_folders(&self) -> Result<Vec<PathBuf>, PortError> {
        Ok(Vec::new())
    }
}

struct Quiet;
impl TidyObserver for Quiet {
    fn progress(&self, _: &TidyProgress) {}
    fn is_cancelled(&self) -> bool {
        false
    }
}

#[test]
fn identity_tells_hard_links_apart_from_copies_and_reads_without_following_links() {
    let dir = TempDir::new().expect("temp dir");
    let one = dir.path().join("one.bin");
    let linked = dir.path().join("linked.bin");
    let copy = dir.path().join("copy.bin");
    fs::write(&one, b"hello world").expect("write");
    fs::hard_link(&one, &linked).expect("hard link");
    fs::write(&copy, b"hello world").expect("write");
    let adapter = WinFileSystem::new();

    let a = adapter.identity(&one).expect("identity");
    let b = adapter.identity(&linked).expect("identity");
    let c = adapter.identity(&copy).expect("identity");
    assert_eq!((a.volume, a.file_id), (b.volume, b.file_id));
    assert_ne!((a.volume, a.file_id), (c.volume, c.file_id));
    assert_eq!(a.links, 2);
    assert!(a.on_disk);

    let mut buf = [0_u8; 5];
    assert_eq!(adapter.read_at(&one, 6, &mut buf).expect("read"), 5);
    assert_eq!(&buf, b"world");
    assert_eq!(adapter.read_at(&one, 11, &mut buf).expect("read"), 0);
    assert!(matches!(
        adapter.identity(dir.path()),
        Err(PortError::InvalidInput(_))
    ));
    assert!(matches!(
        adapter.read_at(&dir.path().join("missing"), 0, &mut buf),
        Err(PortError::NotFound(_))
    ));
}

#[test]
fn the_real_adapter_finds_duplicates_empty_folders_and_broken_shortcuts() {
    let dir = TempDir::new().expect("temp dir");
    let root = dir.path();
    let body = vec![7_u8; 200_000];
    fs::write(root.join("a.bin"), &body).expect("write");
    fs::create_dir(root.join("sub")).expect("mkdir");
    fs::write(root.join("sub").join("b.bin"), &body).expect("write");
    fs::hard_link(root.join("a.bin"), root.join("a-link.bin")).expect("hard link");
    fs::write(root.join("other.bin"), vec![8_u8; 200_000]).expect("write");
    fs::create_dir_all(root.join("empty").join("inner")).expect("mkdir");

    let finder = TidyFinder::new(
        Arc::new(WinFileSystem::new()),
        Arc::new(Folders),
        Arc::new(SystemClock),
    );
    let scan = finder
        .scan(
            &[root.to_path_buf()],
            &TidyOptions {
                min_duplicate_bytes: 1,
            },
            &Quiet,
        )
        .expect("scan");
    assert_eq!(scan.duplicates.len(), 1);
    let group = &scan.duplicates[0];
    assert_eq!(group.files.len(), 2, "the hard link is not a third copy");
    assert_eq!(group.hard_links, 1);
    assert_eq!(scan.empty_folders.len(), 1);
    assert_eq!(scan.empty_folders[0].path, root.join("empty"));
    assert_eq!(scan.empty_folders[0].nested, 1);
    assert!(scan.broken_shortcuts.is_empty());
}
