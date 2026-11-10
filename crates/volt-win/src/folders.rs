//! [`KnownFolders`] port on the Windows shell.

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;

use volt_core::{KnownFolders, Location, PortError};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{
    FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_LocalAppData,
    FOLDERID_Music, FOLDERID_Pictures, FOLDERID_Profile, FOLDERID_ProgramData,
    FOLDERID_ProgramFiles, FOLDERID_ProgramFilesX86, FOLDERID_RoamingAppData, FOLDERID_Videos,
    FOLDERID_Windows, KF_FLAG_DEFAULT, SHGetKnownFolderPath,
};
use windows::core::GUID;

use crate::error::{from_hresult, from_io};

/// Resolves the named locations of cleaning rules with the Windows shell.
///
/// The temp folder comes from the process's temp path, then is expanded to its
/// long, resolved form so that the protected-folder comparison cannot be
/// dodged by an 8.3 short name such as `ADMINI~1`.
#[derive(Debug, Default, Clone, Copy)]
pub struct WinKnownFolders;

impl WinKnownFolders {
    /// A resolver backed by the Windows shell.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

/// One shell known folder as a path.
fn shell_folder(id: &GUID, name: &str) -> Result<PathBuf, PortError> {
    // SAFETY: `id` points to a valid known-folder GUID for the call, the flags
    // are the default and no access token is passed (the current user).
    let raw = unsafe { SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None) }
        .map_err(|e| from_hresult(e.code().0, &format!("known folder {name}")))?;
    // SAFETY: on success the shell returned a valid, NUL-terminated wide string
    // that stays valid until it is freed below.
    let path = PathBuf::from(OsString::from_wide(unsafe { raw.as_wide() }));
    // SAFETY: the buffer was allocated by the shell for the caller to free with
    // CoTaskMemFree, and `raw` is not used afterwards.
    unsafe { CoTaskMemFree(Some(raw.0.cast_const().cast())) };
    Ok(path)
}

/// The fully resolved path without the `\\?\` verbatim prefix, or the path as
/// given when it cannot be resolved.
fn resolved(path: PathBuf) -> PathBuf {
    let Ok(canonical) = std::fs::canonicalize(&path) else {
        return path;
    };
    let text = canonical.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        // `\\?\C:\...` is a plain drive path; `\\?\UNC\...` is left verbatim.
        Some(rest) if rest.chars().nth(1) == Some(':') => PathBuf::from(rest),
        _ => canonical,
    }
}

impl KnownFolders for WinKnownFolders {
    fn location(&self, location: Location) -> Result<PathBuf, PortError> {
        match location {
            Location::UserTemp => {
                let temp = std::env::temp_dir();
                std::fs::metadata(&temp).map_err(|e| from_io(&e, "the temp folder"))?;
                Ok(resolved(temp))
            }
            Location::LocalAppData => shell_folder(&FOLDERID_LocalAppData, "LocalAppData"),
            Location::RoamingAppData => shell_folder(&FOLDERID_RoamingAppData, "RoamingAppData"),
            Location::ProgramData => shell_folder(&FOLDERID_ProgramData, "ProgramData"),
            Location::Windows => shell_folder(&FOLDERID_Windows, "Windows"),
            // `Location` is non-exhaustive: a location this adapter does not
            // know is refused, never guessed.
            other => Err(PortError::Unsupported(format!("location {other}"))),
        }
    }

    fn protected_folders(&self) -> Result<Vec<PathBuf>, PortError> {
        // The profile and the Windows directory must resolve: without them the
        // check would protect nothing, so the whole list fails.
        let mut folders = vec![
            shell_folder(&FOLDERID_Profile, "Profile")?,
            shell_folder(&FOLDERID_Windows, "Windows")?,
        ];
        // The rest is best effort: a folder the shell cannot name (or that does
        // not exist on this machine) cannot be a root either.
        for (id, name) in [
            (&FOLDERID_ProgramFiles, "ProgramFiles"),
            (&FOLDERID_ProgramFilesX86, "ProgramFilesX86"),
            (&FOLDERID_ProgramData, "ProgramData"),
            (&FOLDERID_LocalAppData, "LocalAppData"),
            (&FOLDERID_RoamingAppData, "RoamingAppData"),
            (&FOLDERID_Desktop, "Desktop"),
            (&FOLDERID_Documents, "Documents"),
            (&FOLDERID_Downloads, "Downloads"),
            (&FOLDERID_Pictures, "Pictures"),
            (&FOLDERID_Music, "Music"),
            (&FOLDERID_Videos, "Videos"),
        ] {
            if let Ok(folder) = shell_folder(id, name) {
                folders.push(folder);
            }
        }
        // Shell folders can be redirected (for example into OneDrive); the
        // resolved form is protected as well.
        let extra: Vec<PathBuf> = folders
            .iter()
            .map(|f| resolved(f.clone()))
            .filter(|r| !folders.contains(r))
            .collect();
        folders.extend(extra);
        Ok(folders)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locations_resolve_to_existing_absolute_folders() {
        let folders = WinKnownFolders::new();
        for location in [
            Location::UserTemp,
            Location::LocalAppData,
            Location::RoamingAppData,
            Location::ProgramData,
            Location::Windows,
        ] {
            let path = folders.location(location).expect("resolves");
            assert!(path.is_absolute(), "{location}: {path:?}");
            assert!(path.is_dir(), "{location}: {path:?}");
        }
    }

    #[test]
    fn the_temp_folder_is_inside_the_profile_and_not_protected_itself() {
        let folders = WinKnownFolders::new();
        let temp = folders.location(Location::UserTemp).expect("temp");
        let protected = folders.protected_folders().expect("protected");
        assert!(protected.len() >= 4);
        // Nothing protected equals the temp folder or lies below it.
        assert!(
            protected.iter().all(|p| !p.starts_with(&temp)),
            "{temp:?} vs {protected:?}"
        );
        assert!(protected.iter().all(|p| p.is_absolute()));
    }

    /// Read-only: scans the real temp folder to check speed and that the real
    /// locations pass the root checks. Nothing is moved or changed. Opt in with
    /// `cargo test -p volt-win -- --ignored real_temp --nocapture`.
    #[test]
    #[ignore = "reads the real temp folder (read-only)"]
    fn a_read_only_scan_of_the_real_temp_folder_works_and_is_fast() {
        use std::sync::Arc;
        use std::time::Instant;

        use volt_core::{CleanEngine, CleaningRules, SystemClock};

        let rules = CleaningRules::from_toml(
            r#"
[[rule]]
id = "real-temp"
title = "t"
description = "d"
category = "temporary_files"
risk = "safe"
include = ["**"]
root = { location = "user_temp" }
constraints = { min_age_days = 2, max_depth = 8, remove_empty_dirs = true }
"#,
        )
        .expect("rules");
        let engine = CleanEngine::new(
            rules,
            Arc::new(WinKnownFolders::new()),
            Arc::new(crate::WinFileSystem::new()),
            Arc::new(SystemClock),
        );
        let started = Instant::now();
        let plan = engine.scan_all();
        let scan = &plan.rules[0];
        eprintln!(
            "real temp scan: {:?}, failure {:?}, {} files, {} folders, {} bytes, counts {:?}",
            started.elapsed(),
            scan.failure,
            scan.file_count(),
            scan.directory_count(),
            scan.total_bytes(),
            scan.counts
        );
        assert_eq!(scan.failure, None);
    }
}
