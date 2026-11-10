//! The display language of the signed-in user, as Windows reports it.

use windows::Win32::Globalization::{GetUserPreferredUILanguages, MUI_LANGUAGE_NAME};
use windows::core::PWSTR;

/// The user's preferred display languages as BCP 47 names (`vi-VN`, `en-US`), best first.
///
/// This is the list Settings shows under Language and region, Windows display language. It is
/// read-only. An empty list means Windows did not say, and the caller falls back to its default.
#[must_use]
pub fn display_languages() -> Vec<String> {
    let mut count = 0_u32;
    let mut len = 0_u32;
    // SAFETY: the first call passes no buffer and only asks for the length in UTF-16 units that
    // the second needs; `count` and `len` are live locals the call writes through.
    let sized = unsafe {
        GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &raw mut count, None, &raw mut len)
    };
    if sized.is_err() || len == 0 {
        return Vec::new();
    }
    let mut buffer = vec![0_u16; len as usize];
    // SAFETY: `buffer` holds exactly `len` UTF-16 units, the size the first call asked for, and
    // outlives the call; `len` tells Windows how many units it may write.
    let read = unsafe {
        GetUserPreferredUILanguages(
            MUI_LANGUAGE_NAME,
            &raw mut count,
            Some(PWSTR(buffer.as_mut_ptr())),
            &raw mut len,
        )
    };
    if read.is_err() {
        return Vec::new();
    }
    let written = (len as usize).min(buffer.len());
    split_multi_sz(&buffer[..written])
}

/// Splits a Windows multi-string (names separated by NUL, ended by an empty one) into names.
fn split_multi_sz(units: &[u16]) -> Vec<String> {
    units
        .split(|unit| *unit == 0)
        .take_while(|name| !name.is_empty())
        .map(String::from_utf16_lossy)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    #[test]
    fn a_multi_string_is_split_at_nuls_and_ends_at_the_empty_string() {
        let units = wide("vi-VN\0en-US\0\0");
        assert_eq!(split_multi_sz(&units), ["vi-VN", "en-US"]);
        assert!(split_multi_sz(&wide("\0\0")).is_empty());
        assert!(split_multi_sz(&[]).is_empty());
    }

    #[test]
    fn the_names_windows_reports_are_read_without_failing() {
        // Read-only: whatever this machine is set to, every name is a language tag.
        for name in display_languages() {
            assert!(!name.is_empty() && name.is_ascii(), "{name}");
        }
    }
}
