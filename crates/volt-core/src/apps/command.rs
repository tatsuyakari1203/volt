//! The uninstall command an app registered, checked before anything is started.
//!
//! The registry holds one command line per app (`UninstallString`). Volt starts it as the vendor
//! registered it: the program and the arguments exactly as written, with no shell in between and
//! never built from a display name. This module only decides whether the text is fit to start and
//! splits it into the program and the arguments; the adapter does the starting.

use thiserror::Error;

use crate::startup::parse_command_line;

/// The longest command line accepted (in bytes). Real ones are a few hundred.
const MAX_LENGTH: usize = 4096;

/// A registered uninstall command that passed [`UninstallCommand::parse`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UninstallCommand {
    program: String,
    arguments: String,
}

/// Why a registered command is refused. Nothing was started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum CommandRefusal {
    /// There is no command (empty or blank).
    #[error("the uninstall command is empty")]
    Empty,
    /// The text is not a usable command line (control characters, stray quotes, a relative path).
    #[error("the uninstall command is malformed")]
    Malformed,
    /// The program is on a network path.
    #[error("the uninstall command starts a program on a network path")]
    Network,
    /// The program is not an `.exe` (a script, an installer package, a shortcut).
    #[error("the uninstall command does not start an .exe program")]
    NotAnExecutable,
}

impl UninstallCommand {
    /// Checks and splits a registered command line.
    ///
    /// The program is a quoted path, an unquoted path ending in `.exe`, or a bare name such as
    /// `MsiExec.exe`; it must be an absolute drive path, start with an environment variable
    /// reference (`%ProgramFiles%\...`, expanded by the adapter) or have no folder at all (the
    /// adapter looks it up in the Windows system folder only). Network paths, relative paths,
    /// `..`, scripts and anything that is not an `.exe` are refused, and so is text with control
    /// characters.
    pub fn parse(text: &str) -> Result<Self, CommandRefusal> {
        let text = text.trim();
        if text.is_empty() {
            return Err(CommandRefusal::Empty);
        }
        if text.len() > MAX_LENGTH || text.chars().any(char::is_control) {
            return Err(CommandRefusal::Malformed);
        }
        let line = parse_command_line(text).ok_or(CommandRefusal::Malformed)?;
        check_program(&line.program)?;
        Ok(Self {
            program: line.program,
            arguments: line.arguments,
        })
    }

    /// The program as registered, without quotes. May start with `%VAR%`; may be a bare name.
    #[must_use]
    pub fn program(&self) -> &str {
        &self.program
    }

    /// The arguments as registered (trimmed); empty when there are none.
    #[must_use]
    pub fn arguments(&self) -> &str {
        &self.arguments
    }

    /// The folder of the program as written, when it has one. Used to recognise Volt's own
    /// uninstaller; never to start anything.
    #[must_use]
    pub fn program_folder(&self) -> Option<&str> {
        self.program
            .rsplit_once(['\\', '/'])
            .map(|(folder, _)| folder)
    }
}

fn check_program(program: &str) -> Result<(), CommandRefusal> {
    if program.starts_with("\\\\") || program.starts_with("//") {
        return Err(CommandRefusal::Network);
    }
    if program.contains(['"', '<', '>', '|', '*', '?']) {
        return Err(CommandRefusal::Malformed);
    }
    let has_folder = program.contains(['\\', '/']);
    if has_folder {
        let bytes = program.as_bytes();
        let drive_path = bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && (bytes[2] == b'\\' || bytes[2] == b'/');
        if !(drive_path || program.starts_with('%')) {
            return Err(CommandRefusal::Malformed);
        }
        if program.split(['\\', '/']).any(|part| part == "..") {
            return Err(CommandRefusal::Malformed);
        }
    } else if program.contains(':') {
        // `C:run.exe` (drive-relative) and `name:stream` are not a plain file name.
        return Err(CommandRefusal::Malformed);
    }
    let file = program.rsplit(['\\', '/']).next().unwrap_or(program);
    if file.is_empty() {
        return Err(CommandRefusal::Malformed);
    }
    match file.rsplit_once('.') {
        Some((stem, extension)) if stem.is_empty() || !extension.eq_ignore_ascii_case("exe") => {
            Err(CommandRefusal::NotAnExecutable)
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(text: &str) -> (String, String) {
        let command = UninstallCommand::parse(text).expect("accepted");
        (command.program().to_owned(), command.arguments().to_owned())
    }

    #[test]
    fn a_quoted_path_keeps_its_arguments_as_registered() {
        assert_eq!(
            parsed(r#""C:\Program Files\Vendor App\unins000.exe" /SILENT /LOG="C:\a b.txt""#),
            (
                r"C:\Program Files\Vendor App\unins000.exe".to_owned(),
                r#"/SILENT /LOG="C:\a b.txt""#.to_owned()
            )
        );
    }

    #[test]
    fn an_unquoted_path_with_spaces_ends_at_the_exe() {
        assert_eq!(
            parsed(r"C:\Program Files\Vendor App\uninstall.exe -u"),
            (
                r"C:\Program Files\Vendor App\uninstall.exe".to_owned(),
                "-u".to_owned()
            )
        );
    }

    #[test]
    fn a_bare_name_and_an_environment_path_are_accepted() {
        assert_eq!(
            parsed("MsiExec.exe /X{0F0A1D3E-0000-0000-0000-000000000001}"),
            (
                "MsiExec.exe".to_owned(),
                "/X{0F0A1D3E-0000-0000-0000-000000000001}".to_owned()
            )
        );
        assert_eq!(parsed("msiexec /x {GUID}").0, "msiexec");
        assert_eq!(
            parsed(r"%ProgramFiles%\Vendor\remove.exe").0,
            r"%ProgramFiles%\Vendor\remove.exe"
        );
    }

    #[test]
    fn empty_and_blank_commands_are_refused() {
        for text in ["", "   ", "\t"] {
            assert_eq!(UninstallCommand::parse(text), Err(CommandRefusal::Empty));
        }
        assert_eq!(
            UninstallCommand::parse(r#""""#),
            Err(CommandRefusal::Malformed)
        );
    }

    #[test]
    fn malformed_commands_are_refused() {
        for text in [
            "C:\\a.exe\r\n/S",
            "C:\\a.exe\0 /S",
            r"relative\folder\a.exe",
            r"C:\x\..\..\Windows\a.exe",
            "C:a.exe",
            "name:stream.exe",
            "a|b.exe",
        ] {
            assert_eq!(
                UninstallCommand::parse(text),
                Err(CommandRefusal::Malformed),
                "{text:?}"
            );
        }
        let long = format!("C:\\{}.exe", "a".repeat(5000));
        assert_eq!(
            UninstallCommand::parse(&long),
            Err(CommandRefusal::Malformed)
        );
    }

    #[test]
    fn network_paths_scripts_and_packages_are_refused() {
        assert_eq!(
            UninstallCommand::parse(r"\\server\share\remove.exe"),
            Err(CommandRefusal::Network)
        );
        for text in [
            r"C:\x\remove.bat /s",
            r"C:\x\remove.cmd",
            r"C:\x\app.msi",
            r"C:\x\link.lnk",
            r"C:\x\script.vbs",
        ] {
            assert_eq!(
                UninstallCommand::parse(text),
                Err(CommandRefusal::NotAnExecutable),
                "{text}"
            );
        }
    }

    #[test]
    fn the_program_folder_is_what_comes_before_the_file_name() {
        let command = UninstallCommand::parse(r#""C:\Tools\A\x.exe" /u"#).expect("accepted");
        assert_eq!(command.program_folder(), Some(r"C:\Tools\A"));
        let bare = UninstallCommand::parse("msiexec /x").expect("accepted");
        assert_eq!(bare.program_folder(), None);
    }
}
