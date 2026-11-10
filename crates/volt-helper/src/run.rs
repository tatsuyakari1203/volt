//! What `main` does, as a function over the argument list so it has one place to be read.
//!
//! The order matters: the process hardens ITSELF first, before it reads anything it was given;
//! then the arguments, which must be exactly one shape; then the install folder, which must be
//! one no non-administrator can change; then the UI it is bound to; only then the pipe.

use std::path::Path;

use volt_win::helper::args::parse_args;
use volt_win::helper::harden::harden_process;
use volt_win::helper::identity::{
    ProcessIdentity, inspect_current, inspect_process, normalise_path,
};
use volt_win::helper::install::verify_install;

use crate::serve::{ExitReason, Helper, Limits};

/// The UI executable the helper is bound to: the sibling in the verified install folder.
pub const UI_IMAGE_NAME: &str = "volt-desktop.exe";

/// The process exit codes. 0 is a normal end; every other code is a refusal to serve, and none of
/// them leaves anything changed.
pub mod exit {
    /// The session ended normally (bye, disconnect, idle or the hard cap).
    pub const OK: u8 = 0;
    /// The command line is not the one accepted shape.
    pub const USAGE: u8 = 2;
    /// The process could not harden itself.
    pub const HARDENING: u8 = 3;
    /// The install folder, the image or the state folder can be changed by a non-administrator, or
    /// the image is not in the Program Files folder.
    pub const INSTALL: u8 = 4;
    /// The UI process the helper is bound to is not there or is not the sibling executable.
    pub const BOUND_UI: u8 = 5;
    /// The helper's user is not the UI's user (an over-the-shoulder elevation by another account).
    pub const OTHER_USER: u8 = 6;
    /// The pipe could not be created.
    pub const PIPE: u8 = 7;
    /// The pipe failed while serving.
    pub const FAILED: u8 = 8;
}

/// Runs the helper for the argument list `argv` (the program name first) and returns the process
/// exit code. Never call this from a test: it hardens the calling process.
#[must_use]
pub fn run(argv: &[String]) -> u8 {
    #[cfg(not(helper_release))]
    if let Some(code) = crate::debug::run(argv) {
        return code;
    }

    if harden_process().is_err() {
        return exit::HARDENING;
    }
    let Ok(parsed) = parse_args(argv) else {
        return exit::USAGE;
    };
    let Ok(image) = std::env::current_exe() else {
        return exit::INSTALL;
    };
    let Ok(layout) = verify_install(&image) else {
        return exit::INSTALL;
    };
    let bound = match bound_ui(
        &layout.sibling(UI_IMAGE_NAME),
        parsed.ui_pid,
        parsed.ui_created_at_100ns,
    ) {
        Ok(bound) => bound,
        Err(code) => return code,
    };
    let Ok(helper) = Helper::bind(&parsed.pipe, bound) else {
        return exit::PIPE;
    };
    match helper
        .serve(crate::production_session(), &Limits::default())
        .exit
    {
        ExitReason::Failed(_) => exit::FAILED,
        _ => exit::OK,
    }
}

/// Checks the UI process the helper was told it serves: it must exist with the creation time it
/// was given, run the expected sibling image and belong to the helper's own user.
///
/// # Errors
/// The exit code to end with.
pub fn bound_ui(
    expected_image: &Path,
    pid: u32,
    created_at_100ns: u64,
) -> Result<ProcessIdentity, u8> {
    let ui = inspect_process(pid).map_err(|_| exit::BOUND_UI)?;
    let expected = expected_image.to_string_lossy();
    if ui.identity.created_at_100ns != created_at_100ns
        || normalise_path(&ui.identity.image) != normalise_path(&expected)
    {
        return Err(exit::BOUND_UI);
    }
    let own = inspect_current().map_err(|_| exit::HARDENING)?;
    if ui.token.user_sid != own.token.user_sid {
        return Err(exit::OTHER_USER);
    }
    Ok(ProcessIdentity {
        pid,
        created_at_100ns,
        image: expected.into_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bound_ui_must_exist_with_the_given_start_time_and_image() {
        let me = inspect_current().expect("own process");
        let image = std::path::PathBuf::from(&me.identity.image);
        let created = me.identity.created_at_100ns;
        let pid = me.identity.pid;

        // This test process plays the UI: right pid, time and image, same user.
        let bound = bound_ui(&image, pid, created).expect("bound");
        assert_eq!((bound.pid, bound.created_at_100ns), (pid, created));

        // A different creation time (a reused id) is refused.
        assert_eq!(
            bound_ui(&image, pid, created + 1).err(),
            Some(exit::BOUND_UI)
        );
        // Another image than the sibling UI executable is refused.
        let elsewhere = image.with_file_name("not-volt-desktop.exe");
        assert_eq!(
            bound_ui(&elsewhere, pid, created).err(),
            Some(exit::BOUND_UI)
        );
        // A process that does not exist is refused.
        assert_eq!(
            bound_ui(&image, 0x7FFF_FFFE, created).err(),
            Some(exit::BOUND_UI)
        );
    }

    #[test]
    fn the_exit_codes_are_distinct() {
        let codes = [
            exit::OK,
            exit::USAGE,
            exit::HARDENING,
            exit::INSTALL,
            exit::BOUND_UI,
            exit::OTHER_USER,
            exit::PIPE,
            exit::FAILED,
        ];
        let mut sorted = codes.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), codes.len());
        assert_eq!(exit::OTHER_USER, 6, "the decision names code 6");
    }
}
