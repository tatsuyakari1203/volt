//! [`FilePublisher`] port on the version information of a file.

use std::ffi::c_void;
use std::path::{Path, PathBuf};

use volt_core::{FilePublisher, PortError};
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
};
use windows::core::PCWSTR;

use crate::wide::to_wide_nul;

/// Who published a program, read from its version resource (`CompanyName`).
///
/// Read-only: the file is read but never run, and nothing is changed. A file
/// with no version information, a missing file and a path on a network share
/// all read as "no publisher": the caller treats the publisher as optional.
#[derive(Debug, Default, Clone, Copy)]
pub struct WinFilePublisher;

impl WinFilePublisher {
    /// A publisher reader backed by the version API.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

/// Replaces `%NAME%` references with the value of the environment variable,
/// ignoring case. A reference to a variable that is not set is left as
/// written, and so is a lone `%`.
pub(crate) fn expand_environment(text: &str, lookup: &dyn Fn(&str) -> Option<String>) -> String {
    let mut expanded = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('%') {
        expanded.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) if end > 0 => {
                let name = &after[..end];
                if let Some(value) = lookup(name) {
                    expanded.push_str(&value);
                } else {
                    expanded.push('%');
                    expanded.push_str(name);
                    expanded.push('%');
                }
                rest = &after[end + 1..];
            }
            _ => {
                expanded.push('%');
                rest = after;
            }
        }
    }
    expanded.push_str(rest);
    expanded
}

/// The file to read for a program as written in a startup command: environment
/// variables expanded, and a bare name looked up in the Windows system folder
/// only. Network paths are refused (reading one could hang, or send the user's
/// credentials to another machine).
fn resolve(executable: &str) -> Option<PathBuf> {
    let expanded = expand_environment(executable.trim(), &|name| std::env::var(name).ok());
    if expanded.starts_with(r"\\") || expanded.starts_with("//") || expanded.contains('%') {
        return None;
    }
    let path = PathBuf::from(&expanded);
    if path.is_absolute() {
        return Some(path);
    }
    // A bare name such as `rundll32.exe`: the system folder, nothing else.
    let mut file = PathBuf::from(&expanded);
    if file.components().count() != 1 {
        return None;
    }
    if file.extension().is_none() {
        file.set_extension("exe");
    }
    let root = std::env::var_os("SystemRoot")?;
    Some(Path::new(&root).join("System32").join(file))
}

/// Reads `CompanyName` from a version resource block.
fn company_name(block: &[u8]) -> Option<String> {
    // The block lists its translations as (language, code page) pairs.
    let mut candidates: Vec<(u16, u16)> = query(block, "\\VarFileInfo\\Translation")
        .map(|bytes| {
            bytes
                .chunks_exact(4)
                .map(|pair| {
                    (
                        u16::from_le_bytes([pair[0], pair[1]]),
                        u16::from_le_bytes([pair[2], pair[3]]),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    // Common fallbacks: US English in Unicode and in Windows-1252, and neutral.
    candidates.extend([(0x0409, 0x04B0), (0x0409, 0x04E4), (0x0000, 0x04B0)]);
    for (language, code_page) in candidates {
        let path = format!("\\StringFileInfo\\{language:04X}{code_page:04X}\\CompanyName");
        let Some(bytes) = query(block, &path) else {
            continue;
        };
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .take_while(|unit| *unit != 0)
            .collect();
        let text = String::from_utf16_lossy(&units).trim().to_owned();
        if !text.is_empty() {
            return Some(text);
        }
    }
    None
}

/// The bytes `VerQueryValueW` finds at `sub_block`, copied out of the block.
/// For a string value the length counts UTF-16 units, so it is doubled.
fn query(block: &[u8], sub_block: &str) -> Option<Vec<u8>> {
    let wide = to_wide_nul(sub_block, "version sub-block").ok()?;
    let mut data: *mut c_void = std::ptr::null_mut();
    let mut len = 0_u32;
    // SAFETY: `block` is a version resource returned by GetFileVersionInfoW and
    // outlives the call; `wide` is a NUL-terminated UTF-16 buffer; `data` and
    // `len` are valid out-pointers. On success `data` points into `block`.
    let found = unsafe {
        VerQueryValueW(
            block.as_ptr().cast(),
            PCWSTR(wide.as_ptr()),
            &raw mut data,
            &raw mut len,
        )
    };
    if !found.as_bool() || data.is_null() || len == 0 {
        return None;
    }
    let is_string = sub_block.contains("\\StringFileInfo\\");
    let byte_len = if is_string {
        len as usize * 2
    } else {
        len as usize
    };
    // The pointer must lie inside `block` and so must the whole range.
    let start = (data as usize).checked_sub(block.as_ptr() as usize)?;
    let end = start.checked_add(byte_len)?;
    block.get(start..end).map(<[u8]>::to_vec)
}

impl FilePublisher for WinFilePublisher {
    fn publisher(&self, executable: &str) -> Result<Option<String>, PortError> {
        let Some(path) = resolve(executable) else {
            return Ok(None);
        };
        let wide = to_wide_nul(&path.to_string_lossy(), "program path")?;
        // SAFETY: `wide` is a NUL-terminated UTF-16 buffer that outlives the
        // call; the optional handle argument is passed as None.
        let size = unsafe { GetFileVersionInfoSizeW(PCWSTR(wide.as_ptr()), None) };
        if size == 0 {
            // No version resource, no such file, or a file that is not a program.
            return Ok(None);
        }
        let mut block = vec![0_u8; size as usize];
        // SAFETY: `wide` is a NUL-terminated UTF-16 buffer that outlives the
        // call; `block` is valid for `size` bytes of writes.
        let read = unsafe {
            GetFileVersionInfoW(PCWSTR(wide.as_ptr()), None, size, block.as_mut_ptr().cast())
        };
        if read.is_err() {
            return Ok(None);
        }
        Ok(company_name(&block))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(name: &str) -> Option<String> {
        match name.to_ascii_lowercase().as_str() {
            "programfiles" => Some(r"C:\Program Files".to_owned()),
            "x" => Some("y".to_owned()),
            _ => None,
        }
    }

    #[test]
    fn environment_references_are_expanded_and_unknown_ones_left_alone() {
        assert_eq!(
            expand_environment(r"%ProgramFiles%\Sample\a.exe", &env),
            r"C:\Program Files\Sample\a.exe"
        );
        assert_eq!(expand_environment("%X%%x%", &env), "yy");
        assert_eq!(expand_environment(r"%NOPE%\a", &env), r"%NOPE%\a");
        assert_eq!(expand_environment("100% sure", &env), "100% sure");
        assert_eq!(expand_environment("%%", &env), "%%");
        assert_eq!(expand_environment("plain", &env), "plain");
        assert_eq!(expand_environment("%x", &env), "%x");
    }

    #[test]
    fn network_paths_and_unresolved_references_are_not_read() {
        assert_eq!(resolve(r"\\server\share\a.exe"), None);
        assert_eq!(resolve("//server/share/a.exe"), None);
        assert_eq!(resolve(r"%DefinitelyNotSet12345%\a.exe"), None);
        assert_eq!(resolve(r"sub\dir\a.exe"), None);
    }

    #[test]
    fn a_bare_name_is_looked_up_in_the_system_folder_only() {
        let path = resolve("rundll32").expect("resolves");
        assert!(path.ends_with(r"System32\rundll32.exe"), "{path:?}");
        assert_eq!(
            resolve(r"C:\Tools\a.exe"),
            Some(PathBuf::from(r"C:\Tools\a.exe"))
        );
    }

    #[test]
    fn a_system_program_has_a_publisher_and_a_missing_file_or_a_text_file_has_none() {
        let reader = WinFilePublisher::new();
        // Read-only: asks a program that ships with Windows about itself.
        let publisher = reader
            .publisher(r"%SystemRoot%\System32\notepad.exe")
            .expect("reads");
        assert!(
            publisher.is_some_and(|p| p.contains("Microsoft")),
            "notepad.exe should name Microsoft"
        );
        assert_eq!(
            reader.publisher(r"C:\this\does\not\exist\anything.exe"),
            Ok(None)
        );
        // A plain text file has no version resource.
        let dir = tempfile::tempdir().expect("temp dir");
        let file = dir.path().join("plain.exe");
        std::fs::write(&file, "not a program").expect("write");
        assert_eq!(reader.publisher(&file.to_string_lossy()), Ok(None));
    }
}
