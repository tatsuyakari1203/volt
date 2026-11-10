//! Translation of Win32 error codes into [`PortError`].

use volt_core::PortError;

const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_PATH_NOT_FOUND: u32 = 3;
const ERROR_ACCESS_DENIED: u32 = 5;
const ERROR_INVALID_NAME: u32 = 123;
const ERROR_PRIVILEGE_NOT_HELD: u32 = 1314;
const ERROR_SERVICE_DOES_NOT_EXIST: u32 = 1060;
const ERROR_INVALID_SERVICE_ACCOUNT: u32 = 1057;

/// Maps a Win32 error code to a typed error. `context` names what was being
/// done (for example the value or service), so the message is useful on its own.
///
/// Access denied and missing privilege become [`PortError::AccessDenied`],
/// not-found codes become [`PortError::NotFound`], everything else keeps the OS
/// code and message in [`PortError::Other`].
pub(crate) fn from_win32(code: u32, context: &str) -> PortError {
    match code {
        ERROR_ACCESS_DENIED | ERROR_PRIVILEGE_NOT_HELD => {
            PortError::AccessDenied(format!("{context}: {}", os_message(code)))
        }
        ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND | ERROR_SERVICE_DOES_NOT_EXIST => {
            PortError::NotFound(context.to_owned())
        }
        ERROR_INVALID_NAME | ERROR_INVALID_SERVICE_ACCOUNT => {
            PortError::InvalidInput(format!("{context}: {}", os_message(code)))
        }
        _ => PortError::Other(format!("{context}: {}", os_message(code))),
    }
}

/// Maps an HRESULT that wraps a Win32 code (`0x8007xxxx`) like [`from_win32`];
/// other HRESULTs keep their number in an [`PortError::Other`].
pub(crate) fn from_hresult(hr: i32, context: &str) -> PortError {
    let bits = hr.cast_unsigned();
    if bits >> 16 == 0x8007 {
        from_win32(bits & 0xFFFF, context)
    } else {
        PortError::Other(format!("{context}: HRESULT 0x{bits:08X}"))
    }
}

/// Maps an I/O error from `std`, which carries the Win32 code when it came
/// from the OS.
pub(crate) fn from_io(error: &std::io::Error, context: &str) -> PortError {
    match error.raw_os_error().map(i32::cast_unsigned) {
        Some(code) => from_win32(code, context),
        None => PortError::Other(format!("{context}: {error}")),
    }
}

fn os_message(code: u32) -> String {
    std::io::Error::from_raw_os_error(code.cast_signed()).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_denied_and_missing_privilege_are_typed() {
        assert!(matches!(from_win32(5, "x"), PortError::AccessDenied(_)));
        assert!(matches!(from_win32(1314, "x"), PortError::AccessDenied(_)));
    }

    #[test]
    fn not_found_codes_are_typed() {
        for code in [2, 3, 1060] {
            assert_eq!(
                from_win32(code, "thing"),
                PortError::NotFound("thing".into())
            );
        }
    }

    #[test]
    fn other_codes_keep_code_and_message() {
        let PortError::Other(text) = from_win32(32, "file") else {
            panic!("expected Other");
        };
        assert!(text.starts_with("file: "));
        assert!(text.contains("os error 32"), "{text}");
    }

    #[test]
    fn hresult_wrapping_win32_is_unwrapped() {
        assert!(matches!(
            from_hresult(0x8007_0005_u32.cast_signed(), "x"),
            PortError::AccessDenied(_)
        ));
        assert!(matches!(
            from_hresult(0x8000_4005_u32.cast_signed(), "x"),
            PortError::Other(_)
        ));
    }
}
