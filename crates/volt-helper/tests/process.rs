//! The helper BINARY as a child process, unelevated (the dev build has no administrator manifest).
//!
//! These check what only a real process can show: that a bad command line ends the process before
//! it does any work, that a helper that is not installed in the Program Files folder refuses to
//! serve, and what the real hardening leaves behind. They never start anything elevated and never
//! touch memory lists.

#![cfg(all(windows, not(helper_release)))]

use std::process::{Command, Output};

use volt_win::helper::args::PipeName;
use volt_win::helper::harden::KEPT_PRIVILEGES;
use volt_win::helper::pipe::{OpenError, PipeClient};

const HELPER: &str = env!("CARGO_BIN_EXE_volt-helper");

fn run(args: &[&str]) -> Output {
    Command::new(HELPER).args(args).output().expect("run")
}

const NAME: &str = "volt-0123456789abcdef0123456789abcdef";

#[test]
fn any_command_line_but_the_one_shape_ends_the_process_with_the_usage_code() {
    for args in [
        &[][..],
        &["serve"],
        &["serve", "--pipe", NAME],
        &["--debug"],
        &["run", "--pipe", NAME, "--ui-pid", "4", "--ui-created", "5"],
        // Right shape, one argument too many.
        &[
            "serve",
            "--pipe",
            NAME,
            "--ui-pid",
            "4",
            "--ui-created",
            "5",
            "--debug",
        ],
        // Right shape, a pipe name that is not 128 random bits.
        &[
            "serve",
            "--pipe",
            "volt-1234",
            "--ui-pid",
            "4",
            "--ui-created",
            "5",
        ],
        &[
            "serve",
            "--pipe",
            "..\\evil",
            "--ui-pid",
            "4",
            "--ui-created",
            "5",
        ],
        // Numbers that are not canonical.
        &[
            "serve",
            "--pipe",
            NAME,
            "--ui-pid",
            "04",
            "--ui-created",
            "5",
        ],
        &[
            "serve",
            "--pipe",
            NAME,
            "--ui-pid",
            "4",
            "--ui-created",
            "-5",
        ],
    ] {
        let output = run(args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(
            output.stdout.is_empty() && output.stderr.is_empty(),
            "{args:?}"
        );
    }
}

#[test]
fn a_helper_that_is_not_installed_in_program_files_refuses_to_serve() {
    // A valid command line, started from the build folder: the install check refuses it before
    // anything else happens, and in particular before a pipe exists.
    let pipe = PipeName::random().expect("name");
    let output = run(&[
        "serve",
        "--pipe",
        pipe.as_str(),
        "--ui-pid",
        &std::process::id().to_string(),
        "--ui-created",
        "133800000000000000",
    ]);
    assert_eq!(output.status.code(), Some(4));
    assert_eq!(PipeClient::open(&pipe).err(), Some(OpenError::NotThere));
}

#[test]
fn hardening_leaves_a_scrubbed_environment_system32_and_three_privileges() {
    let output = Command::new(HELPER)
        .arg("harden-selftest")
        // Something a hostile parent might plant: it must not survive.
        .env("VOLT_PLANTED", "1")
        .env("COR_ENABLE_PROFILING", "1")
        .output()
        .expect("run");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let text = String::from_utf8_lossy(&output.stdout);
    let field = |name: &str| -> Vec<String> {
        text.lines()
            .find_map(|line| line.strip_prefix(&format!("{name}=")))
            .map(|value| {
                value
                    .split(',')
                    .filter(|part| !part.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    };

    // Environment: only the three Windows variables (drive-directory entries that start with `=`
    // are not variables a program sets or reads).
    let variables = field("variables");
    for name in &variables {
        assert!(
            name.starts_with('=')
                || ["SystemRoot", "windir", "SystemDrive"]
                    .iter()
                    .any(|kept| kept.eq_ignore_ascii_case(name)),
            "variable {name} survived the scrub"
        );
    }
    assert!(
        variables
            .iter()
            .all(|name| !name.eq_ignore_ascii_case("VOLT_PLANTED"))
    );

    // Working directory: System32.
    let directory = field("directory");
    assert!(
        directory
            .first()
            .is_some_and(|dir| dir.to_lowercase().ends_with("\\system32")),
        "{directory:?}"
    );

    // Privileges: nothing outside the three the helper keeps.
    let privileges = field("privileges");
    assert!(!privileges.is_empty(), "the child reported no privileges");
    for name in &privileges {
        assert!(
            KEPT_PRIVILEGES
                .iter()
                .any(|kept| kept.eq_ignore_ascii_case(name)),
            "privilege {name} survived"
        );
    }
    assert!(
        privileges
            .iter()
            .any(|name| name.eq_ignore_ascii_case("SeChangeNotifyPrivilege")),
        "{privileges:?}"
    );
}
