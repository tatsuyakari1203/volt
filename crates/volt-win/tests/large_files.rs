//! The large files finder against the real file system adapter, over a temp directory. Scan only:
//! moving files would put them in the real Recycle Bin, which tests do not do.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use tempfile::TempDir;
use volt_core::{
    KnownFolders, LargeFilesFinder, LargeFilesObserver, Location, PortError, SystemClock,
};
use volt_win::WinFileSystem;

struct Folders(Vec<PathBuf>);

impl KnownFolders for Folders {
    fn location(&self, location: Location) -> Result<PathBuf, PortError> {
        Err(PortError::NotFound(format!("{location}")))
    }

    fn protected_folders(&self) -> Result<Vec<PathBuf>, PortError> {
        Ok(Vec::new())
    }

    fn system_folders(&self) -> Result<Vec<PathBuf>, PortError> {
        Ok(self.0.clone())
    }
}

struct Quiet;
impl LargeFilesObserver for Quiet {
    fn progress(&self, _: u64, _: u64) {}
    fn is_cancelled(&self) -> bool {
        false
    }
}

#[test]
fn the_real_adapter_lists_the_biggest_files_and_skips_system_folders() {
    let dir = TempDir::new().expect("temp dir");
    let base = dir.path();
    fs::create_dir_all(base.join("Windows")).expect("mkdir");
    fs::create_dir_all(base.join("data/deep")).expect("mkdir");
    fs::write(base.join("Windows/huge.dll"), vec![0_u8; 5000]).expect("write");
    fs::write(base.join("pagefile.sys"), vec![0_u8; 5000]).expect("write");
    fs::write(base.join("data/big.iso"), vec![0_u8; 3000]).expect("write");
    fs::write(base.join("data/deep/small.txt"), vec![0_u8; 10]).expect("write");

    let finder = LargeFilesFinder::new(
        Arc::new(WinFileSystem::new()),
        Arc::new(Folders(vec![base.join("Windows")])),
        Arc::new(SystemClock),
    );
    let scan = finder.scan(base, 10, &Quiet).expect("scan");
    let names: Vec<String> = scan
        .files
        .iter()
        .map(|f| {
            f.path
                .file_name()
                .expect("name")
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(names, ["big.iso", "small.txt"]);
    assert_eq!(scan.files[0].size_bytes, 3000);
    assert!(scan.files[0].modified.is_some());
    assert_eq!(scan.excluded, 2);
}
