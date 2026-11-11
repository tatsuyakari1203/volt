//! winget: runs the Windows Package Manager for the update listing and for one update at a time.
//!
//! [`WinWinget`] starts `winget.exe` from the per-user App Execution Alias folder with the fixed
//! argument list of a [`WingetCommand`], never through a shell, with no console window and a null
//! standard input. Its output is read up to a cap. An argument that would accept an agreement is
//! refused here as well, whatever the caller built. The installer an update starts shows its own
//! window and any administrator prompt; only winget itself is waited for, and only winget is
//! stopped when the wait runs out.

use std::io::Read;
use std::os::windows::process::CommandExt as _;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};

use volt_core::{PortError, Winget, WingetCommand, WingetRun};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// The most output kept.
const MAX_OUTPUT: u64 = 1024 * 1024;
/// How long the listing may take.
const LIST_LIMIT: Duration = Duration::from_secs(120);
/// How long one update may take before Volt stops waiting for winget.
const UPGRADE_LIMIT: Duration = Duration::from_secs(45 * 60);

/// The Windows Package Manager.
#[derive(Debug, Default, Clone, Copy)]
pub struct WinWinget;

impl WinWinget {
    /// A runner for the winget of the current user.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

fn program() -> Result<PathBuf, PortError> {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .ok_or_else(|| PortError::NotFound("winget".into()))?;
    let path = base
        .join("Microsoft")
        .join("WindowsApps")
        .join("winget.exe");
    if path.is_file() {
        Ok(path)
    } else {
        Err(PortError::NotFound("winget".into()))
    }
}

/// Refuses anything that could accept an agreement on the user's behalf.
fn checked(args: &[String]) -> Result<(), PortError> {
    if args.first().map(String::as_str) != Some("upgrade")
        || args
            .iter()
            .any(|arg| arg.to_lowercase().contains("accept") || arg.contains('\0'))
    {
        return Err(PortError::InvalidInput(
            "a winget argument is not allowed".into(),
        ));
    }
    Ok(())
}

impl Winget for WinWinget {
    fn run(&self, command: &WingetCommand) -> Result<WingetRun, PortError> {
        let args = command.args();
        checked(&args)?;
        let limit = match command {
            WingetCommand::List => LIST_LIMIT,
            WingetCommand::Upgrade { .. } => UPGRADE_LIMIT,
        };
        let path = program()?;
        let (reader, writer) = std::io::pipe()
            .map_err(|e| PortError::Other(format!("could not create a pipe: {e}")))?;
        let writer_err = writer
            .try_clone()
            .map_err(|e| PortError::Other(format!("could not create a pipe: {e}")))?;
        let mut process = std::process::Command::new(path);
        process
            .args(&args)
            .stdin(Stdio::null())
            .stdout(writer)
            .stderr(writer_err)
            .creation_flags(CREATE_NO_WINDOW);
        let spawned = process.spawn();
        // The command holds the parent's copies of the pipe's write ends; close them so the reader
        // sees the end of the output.
        drop(process);
        let mut child = spawned.map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => PortError::NotFound("winget".into()),
            std::io::ErrorKind::PermissionDenied => PortError::AccessDenied("winget".into()),
            _ => PortError::Other(format!("could not start winget: {e}")),
        })?;
        let collector = std::thread::spawn(move || {
            let mut reader = reader;
            let mut bytes = Vec::new();
            let _ = (&mut reader).take(MAX_OUTPUT).read_to_end(&mut bytes);
            // Drain the rest so winget never blocks on a full pipe.
            let _ = std::io::copy(&mut reader, &mut std::io::sink());
            bytes
        });
        let started = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if started.elapsed() >= limit => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(PortError::TimedOut("winget".into()));
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(100)),
                Err(e) => return Err(PortError::Other(format!("could not wait for winget: {e}"))),
            }
        };
        let bytes = collector.join().unwrap_or_default();
        let text = String::from_utf8_lossy(&bytes);
        Ok(WingetRun {
            exit_code: status.code().unwrap_or(-1),
            output: text.trim_start_matches('\u{feff}').to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_argument_that_accepts_an_agreement_is_refused() {
        let args = |list: &[&str]| list.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>();
        assert!(checked(&args(&["upgrade", "--disable-interactivity"])).is_ok());
        assert!(checked(&args(&["upgrade", "--accept-source-agreements"])).is_err());
        assert!(checked(&args(&["install", "x"])).is_err());
    }
}
