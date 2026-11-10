//! Turning the text of a startup command into a program and its arguments.
//!
//! A Run value holds a command line as one string. Windows starts it the way
//! `CreateProcess` would, which allows a quoted path, an unquoted path without
//! spaces and an unquoted path *with* spaces followed by arguments. Reading the
//! program out of it is pure string work, so it lives in the core and is tested
//! without a machine. Environment variable references such as `%ProgramFiles%`
//! are **left as written**: expanding them needs the environment, which only an
//! adapter may read.

/// Extensions that end a program path when it is not quoted. Without a quote
/// the end of `C:\Program Files\X\x.exe /silent` can only be found by looking
/// for the file extension.
const PROGRAM_EXTENSIONS: [&str; 6] = [".exe", ".com", ".bat", ".cmd", ".lnk", ".scr"];

/// A command line split into its program and its arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandLine {
    /// The program as written, without quotes. Environment variable references
    /// are not expanded.
    pub program: String,
    /// Everything after the program, trimmed. Empty when there are none.
    pub arguments: String,
}

/// Splits a startup command into its program and arguments.
///
/// - A leading quote starts a quoted program: it ends at the next quote (or at
///   the end of the text when the quote is never closed).
/// - Otherwise the program ends at the first whitespace that follows a known
///   program extension (`.exe`, `.com`, `.bat`, `.cmd`, `.lnk`, `.scr`), which
///   handles paths with spaces; with no such extension it is the first
///   whitespace-delimited word (`cmd /c ...` gives `cmd`).
///
/// Returns `None` for an empty or blank command, and for one whose program
/// would be empty (`""`).
#[must_use]
pub fn parse_command_line(text: &str) -> Option<CommandLine> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let (program, rest) = if let Some(quoted) = text.strip_prefix('"') {
        match quoted.find('"') {
            Some(end) => (&quoted[..end], &quoted[end + 1..]),
            None => (quoted, ""),
        }
    } else {
        split_unquoted(text)
    };
    let program = program.trim();
    if program.is_empty() {
        return None;
    }
    Some(CommandLine {
        program: program.to_owned(),
        arguments: rest.trim().to_owned(),
    })
}

/// Splits an unquoted command at the end of its program path.
fn split_unquoted(text: &str) -> (&str, &str) {
    // Candidate ends: every whitespace position, then the end of the text.
    let boundaries = text
        .char_indices()
        .filter(|(_, c)| c.is_whitespace())
        .map(|(index, _)| index)
        .chain(std::iter::once(text.len()));
    for end in boundaries {
        let candidate = text[..end].to_ascii_lowercase();
        if PROGRAM_EXTENSIONS
            .iter()
            .any(|extension| candidate.ends_with(extension))
        {
            return (&text[..end], &text[end..]);
        }
    }
    // No known extension: the first word.
    match text.find(char::is_whitespace) {
        Some(end) => (&text[..end], &text[end..]),
        None => (text, ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(text: &str) -> (String, String) {
        let line = parse_command_line(text).expect("parses");
        (line.program, line.arguments)
    }

    #[test]
    fn a_quoted_program_ends_at_the_closing_quote() {
        assert_eq!(
            parsed(r#""C:\Program Files\Sample App\app.exe" --background"#),
            (
                r"C:\Program Files\Sample App\app.exe".into(),
                "--background".into()
            )
        );
        assert_eq!(
            parsed(r#""C:\x\y.exe""#),
            (r"C:\x\y.exe".into(), String::new())
        );
        // Spaces and quotes inside the arguments belong to the arguments.
        assert_eq!(
            parsed(r#""C:\x\y.exe" -f "a b" -q"#),
            (r"C:\x\y.exe".into(), r#"-f "a b" -q"#.into())
        );
    }

    #[test]
    fn an_unclosed_quote_takes_the_rest_as_the_program() {
        assert_eq!(
            parsed(r#""C:\x\y.exe /s"#),
            (r"C:\x\y.exe /s".into(), String::new())
        );
    }

    #[test]
    fn an_unquoted_path_without_spaces_ends_at_the_first_space() {
        assert_eq!(
            parsed(r"C:\Tools\sync.exe /minimized"),
            (r"C:\Tools\sync.exe".into(), "/minimized".into())
        );
        assert_eq!(
            parsed(r"C:\Tools\sync.exe"),
            (r"C:\Tools\sync.exe".into(), String::new())
        );
    }

    #[test]
    fn an_unquoted_path_with_spaces_ends_after_the_program_extension() {
        assert_eq!(
            parsed(r"C:\Program Files\Sample App\app.exe -tray"),
            (
                r"C:\Program Files\Sample App\app.exe".into(),
                "-tray".into()
            )
        );
        assert_eq!(
            parsed(r"C:\Program Files\Sample App\RUN.BAT"),
            (r"C:\Program Files\Sample App\RUN.BAT".into(), String::new())
        );
    }

    #[test]
    fn a_command_without_a_known_extension_gives_its_first_word() {
        assert_eq!(
            parsed("cmd /c start something"),
            ("cmd".into(), "/c start something".into())
        );
        assert_eq!(parsed("powershell"), ("powershell".into(), String::new()));
    }

    #[test]
    fn environment_variables_are_left_unexpanded() {
        assert_eq!(
            parsed(r#""%ProgramFiles%\Sample\app.exe" /s"#),
            (r"%ProgramFiles%\Sample\app.exe".into(), "/s".into())
        );
        assert_eq!(
            parsed(r"%LOCALAPPDATA%\Sample\update.exe --check"),
            (r"%LOCALAPPDATA%\Sample\update.exe".into(), "--check".into())
        );
    }

    #[test]
    fn blank_commands_and_empty_programs_give_nothing() {
        for text in ["", "   ", "\t\n", r#""""#, r#""" /s"#] {
            assert_eq!(parse_command_line(text), None, "{text:?}");
        }
    }

    #[test]
    fn surrounding_whitespace_is_ignored() {
        assert_eq!(
            parsed("  \"C:\\a b\\c.exe\"   -x  "),
            (r"C:\a b\c.exe".into(), "-x".into())
        );
    }

    #[test]
    fn rundll32_style_commands_keep_their_arguments() {
        assert_eq!(
            parsed(r"rundll32.exe shell32.dll,Control_RunDLL"),
            ("rundll32.exe".into(), "shell32.dll,Control_RunDLL".into())
        );
    }
}
