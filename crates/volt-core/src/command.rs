//! External commands: model and port.
//!
//! A [`Command`] is a program and an argument list, never a shell string, so
//! nothing is ever interpreted by a shell and no quoting can go wrong. The
//! [`Commands`] port runs one and reports its exit code. Commands only run
//! through an [`Operation`](crate::Operation), which is journaled.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::PortError;

/// A program and its arguments. Run directly, never through a shell.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Command {
    /// The executable, such as `powercfg`.
    pub program: String,
    /// The arguments, one list element each. They are passed as they are.
    #[serde(default)]
    pub args: Vec<String>,
    /// How long the command may run, in seconds, when it needs a different limit than the
    /// adapter's default. The adapter never allows more than [`MAX_TIMEOUT_SECS`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u32>,
}

/// The longest time, in seconds, a command may ask for with [`Command::timeout_secs`].
pub const MAX_TIMEOUT_SECS: u32 = 600;

impl Command {
    /// A command with the given program and arguments.
    #[must_use]
    pub fn new<I, S>(program: impl Into<String>, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
            timeout_secs: None,
        }
    }

    /// The same command with its own time limit in seconds (capped at [`MAX_TIMEOUT_SECS`] by
    /// the adapter).
    #[must_use]
    pub const fn with_timeout_secs(mut self, secs: u32) -> Self {
        self.timeout_secs = Some(secs);
        self
    }
}

impl fmt::Display for Command {
    /// For humans (previews, error messages) only: arguments with spaces are
    /// quoted. The command itself is never built from this text.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.program)?;
        for arg in &self.args {
            if arg.is_empty() || arg.contains(char::is_whitespace) {
                write!(f, " \"{arg}\"")?;
            } else {
                write!(f, " {arg}")?;
            }
        }
        Ok(())
    }
}

/// What a finished command left behind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandOutput {
    /// The process exit code. `0` is success by convention.
    pub exit_code: i32,
    /// Standard output and standard error as text, for error messages. The
    /// adapter decides how much it keeps.
    pub output: String,
}

/// Port: run external programs.
pub trait Commands: Send + Sync {
    /// Runs `command` to completion. A program that cannot be started is
    /// [`PortError::NotFound`] (or another error); a program that ran and
    /// exited with a non-zero code is `Ok` with that code.
    fn run(&self, command: &Command) -> Result<CommandOutput, PortError>;
}

/// Runs `command` and requires exit code `0`.
pub(crate) fn run_expecting_success(
    ports: &crate::ports::Ports,
    command: &Command,
) -> Result<(), PortError> {
    let result = ports.commands.run(command)?;
    if result.exit_code == 0 {
        Ok(())
    } else {
        Err(failure(command, &result))
    }
}

/// Runs a detect command: `0` means the effect is in place, `1` means it is
/// not, anything else is an error (the check itself failed).
pub(crate) fn run_detect(
    ports: &crate::ports::Ports,
    command: &Command,
) -> Result<bool, PortError> {
    let result = ports.commands.run(command)?;
    match result.exit_code {
        0 => Ok(true),
        1 => Ok(false),
        _ => Err(failure(command, &result)),
    }
}

fn failure(command: &Command, result: &CommandOutput) -> PortError {
    let output = result.output.trim();
    if output.is_empty() {
        PortError::Other(format!("`{command}` exited with code {}", result.exit_code))
    } else {
        PortError::Other(format!(
            "`{command}` exited with code {}: {output}",
            result.exit_code
        ))
    }
}
