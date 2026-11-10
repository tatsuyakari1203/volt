//! `volt-helper.exe`: the elevated helper. All of the logic is in the library; see `lib.rs`.
//!
//! The release binary is a GUI-subsystem program (no console window while it runs elevated).

#![cfg(windows)]
#![cfg_attr(helper_release, windows_subsystem = "windows")]

use std::process::ExitCode;

fn main() -> ExitCode {
    // `args_os`, then a strict conversion: an argument that is not valid text is refused with the
    // others, never lossily repaired into something that might parse.
    let Ok(argv) = std::env::args_os()
        .map(std::ffi::OsString::into_string)
        .collect::<Result<Vec<String>, _>>()
    else {
        return ExitCode::from(volt_helper::run::exit::USAGE);
    };
    ExitCode::from(volt_helper::run::run(&argv))
}
