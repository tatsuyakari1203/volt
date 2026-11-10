//! UTF-16 helpers shared by the adapters.

use volt_core::PortError;

/// Encodes `s` as UTF-16 with a terminating NUL. Interior NULs are rejected
/// because the Windows API would silently truncate the string there.
pub(crate) fn to_wide_nul(s: &str, what: &str) -> Result<Vec<u16>, PortError> {
    if s.contains('\0') {
        return Err(PortError::InvalidInput(format!(
            "{what} contains a NUL character"
        )));
    }
    Ok(s.encode_utf16().chain(std::iter::once(0)).collect())
}
