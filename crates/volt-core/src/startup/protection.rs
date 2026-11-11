//! Which startup items are protected, and why a switch is locked.
//!
//! Volt shows everything that starts with Windows, but an item Microsoft publishes is protected
//! by default: its switch stays locked until the user chooses to unlock Windows items, and an item
//! Windows cannot run without is never switchable. The rules are here, in the core, and the
//! manager enforces them when a change is asked for; the window only reflects them.

use super::command::parse_command_line;
use crate::process::is_windows_critical;

/// How an item is protected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum StartupProtection {
    /// Nobody in particular depends on it.
    #[default]
    None,
    /// Microsoft or Windows itself publishes it. Locked until the user unlocks Windows items.
    Microsoft,
    /// Windows cannot run properly without it (the logon machinery, the security stack). Never
    /// switchable.
    Critical,
}

/// Why an item's switch is locked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StartupLock {
    /// Never switchable: Windows needs it.
    Critical,
    /// Applies to every user: changing it needs administrator rights.
    NeedsAdministrator,
    /// Changing it needs the administrator helper (a service, or a task that runs for every
    /// user).
    NeedsHelper,
    /// Volt cannot change this kind of item yet.
    NotYet,
    /// Published by Microsoft. Locked by default; the user can unlock it.
    Microsoft,
}

impl StartupLock {
    /// Whether the user can lift this lock by unlocking Windows items.
    #[must_use]
    pub const fn lifted_by_unlock(self) -> bool {
        matches!(self, Self::Microsoft)
    }
}

/// Services whose names mark them as part of the logon, security or networking base of Windows.
/// Compared ignoring case.
const CRITICAL_SERVICES: [&str; 32] = [
    "rpcss",
    "rpceptmapper",
    "dcomlaunch",
    "lsm",
    "samss",
    "eventlog",
    "plugplay",
    "power",
    "profsvc",
    "gpsvc",
    "brokerinfrastructure",
    "coremessagingregistrar",
    "systemeventsbroker",
    "schedule",
    "usermanager",
    "staterepository",
    "trustedinstaller",
    "windefend",
    "wdnissvc",
    "mpssvc",
    "bfe",
    "cryptsvc",
    "keyiso",
    "dhcp",
    "dnscache",
    "nsi",
    "wscsvc",
    "securityhealthservice",
    "sens",
    "timebrokersvc",
    "lanmanworkstation",
    "audiosrv",
];

/// Run entries by name that guard the machine (the Windows Security tray icon). Only counted when
/// Microsoft publishes the program.
const CRITICAL_ENTRY_NAMES: [&str; 2] = ["securityhealth", "windowsdefender"];

/// Task folders, below `\Microsoft\Windows`, that keep Windows secure and up to date.
const CRITICAL_TASK_FOLDERS: [&str; 4] = [
    r"\microsoft\windows\windows defender\",
    r"\microsoft\windows\windowsupdate\",
    r"\microsoft\windows\updateorchestrator\",
    r"\microsoft\windows\servicing\",
];

/// Whether `publisher` is Microsoft (its company name starts with it).
fn microsoft_publisher(publisher: Option<&str>) -> bool {
    publisher.is_some_and(|name| name.trim().to_lowercase().starts_with("microsoft"))
}

/// The program, lower case, with `/` as `\`.
fn normalized(program: &str) -> String {
    program.trim().to_lowercase().replace('/', "\\")
}

/// Whether the program lives in the Windows folder (as a path or through `%SystemRoot%`).
fn in_windows_folder(program: &str) -> bool {
    let path = normalized(program);
    path.starts_with("%systemroot%\\")
        || path.starts_with("%windir%\\")
        || path.contains(":\\windows\\")
}

fn file_name(program: &str) -> String {
    let path = normalized(program);
    path.rsplit('\\').next().unwrap_or_default().to_owned()
}

/// How a program started from a Run key, a Startup folder, `RunOnce` or a task is protected.
#[must_use]
pub fn classify_program(
    name: &str,
    program: Option<&str>,
    publisher: Option<&str>,
) -> StartupProtection {
    let microsoft = microsoft_publisher(publisher) || program.is_some_and(in_windows_folder);
    if !microsoft {
        return StartupProtection::None;
    }
    let critical_program = program.is_some_and(|program| is_windows_critical(&file_name(program)));
    let critical_name = CRITICAL_ENTRY_NAMES.contains(&name.trim().to_lowercase().as_str());
    if critical_program || critical_name {
        StartupProtection::Critical
    } else {
        StartupProtection::Microsoft
    }
}

/// How an auto-start service is protected. `image_path` is the service's command line.
#[must_use]
pub fn classify_service(
    name: &str,
    image_path: &str,
    publisher: Option<&str>,
) -> StartupProtection {
    let program = parse_command_line(image_path).map(|line| line.program);
    let microsoft =
        microsoft_publisher(publisher) || program.as_deref().is_some_and(in_windows_folder);
    if !microsoft {
        return StartupProtection::None;
    }
    if CRITICAL_SERVICES.contains(&name.trim().to_lowercase().as_str()) {
        StartupProtection::Critical
    } else {
        StartupProtection::Microsoft
    }
}

/// How a logon task is protected, from its path in the library and its program.
#[must_use]
pub fn classify_task(
    path: &str,
    program: Option<&str>,
    publisher: Option<&str>,
) -> StartupProtection {
    let lower = normalized(path);
    let name = lower.rsplit('\\').next().unwrap_or_default();
    let by_program = classify_program(name, program, publisher);
    if CRITICAL_TASK_FOLDERS
        .iter()
        .any(|folder| lower.starts_with(folder))
        || by_program == StartupProtection::Critical
    {
        return StartupProtection::Critical;
    }
    if lower.starts_with("\\microsoft\\") || by_program == StartupProtection::Microsoft {
        StartupProtection::Microsoft
    } else {
        StartupProtection::None
    }
}

/// Whether a task runs for every user (the system account, a group, or with the highest
/// privileges) rather than for the user who owns it.
#[must_use]
pub fn task_is_machine_wide(user_id: &str, elevated: bool) -> bool {
    let id = user_id.trim().to_lowercase();
    let account = id.rsplit('\\').next().unwrap_or_default();
    elevated
        || id.is_empty()
        || matches!(
            account,
            "system" | "local service" | "localservice" | "network service" | "networkservice"
        )
        || id.starts_with("s-1-5-18")
        || id.starts_with("s-1-5-19")
        || id.starts_with("s-1-5-20")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_third_party_program_is_not_protected() {
        assert_eq!(
            classify_program(
                "Sample",
                Some(r"C:\Program Files\Sample\sample.exe"),
                Some("Sample Ltd")
            ),
            StartupProtection::None
        );
        assert_eq!(
            classify_program("Sample", None, None),
            StartupProtection::None
        );
    }

    #[test]
    fn a_microsoft_publisher_or_the_windows_folder_marks_it_microsoft() {
        assert_eq!(
            classify_program(
                "Cloud",
                Some(r"C:\Sample\cloud.exe"),
                Some("Microsoft Corporation")
            ),
            StartupProtection::Microsoft
        );
        assert_eq!(
            classify_program("Tool", Some("%SystemRoot%\\System32\\tool.exe"), None),
            StartupProtection::Microsoft
        );
        assert_eq!(
            classify_program("Tool", Some("C:/Windows/System32/tool.exe"), None),
            StartupProtection::Microsoft
        );
        // A folder that only has `windows` in its name is not the Windows folder.
        assert_eq!(
            classify_program("Tool", Some(r"C:\Users\Sample\windows\tool.exe"), None),
            StartupProtection::None
        );
    }

    #[test]
    fn the_logon_machinery_and_the_security_icon_are_critical() {
        assert_eq!(
            classify_program("Shell", Some(r"C:\Windows\System32\userinit.exe"), None),
            StartupProtection::Microsoft,
            "userinit is not in the process list of critical names"
        );
        assert_eq!(
            classify_program(
                "Any",
                Some(r"C:\Windows\System32\winlogon.exe"),
                Some("Microsoft Corporation")
            ),
            StartupProtection::Critical
        );
        assert_eq!(
            classify_program(
                "SecurityHealth",
                Some(r"%windir%\system32\SecurityHealthSystray.exe"),
                Some("Microsoft Corporation")
            ),
            StartupProtection::Critical
        );
        // The same name from another publisher is just a program.
        assert_eq!(
            classify_program(
                "SecurityHealth",
                Some(r"C:\Sample\a.exe"),
                Some("Sample Ltd")
            ),
            StartupProtection::None
        );
    }

    #[test]
    fn services_are_classified_by_name_and_image() {
        assert_eq!(
            classify_service(
                "Spooler",
                r"C:\Windows\System32\spoolsv.exe",
                Some("Microsoft Corporation")
            ),
            StartupProtection::Microsoft
        );
        assert_eq!(
            classify_service(
                "RpcSs",
                r"C:\Windows\system32\svchost.exe -k rpcss -p",
                None
            ),
            StartupProtection::Critical
        );
        assert_eq!(
            classify_service(
                "SampleSync",
                r#""C:\Program Files\Sample\sync.exe" --service"#,
                Some("Sample Ltd")
            ),
            StartupProtection::None
        );
        // A third-party service that borrows a critical name stays unprotected.
        assert_eq!(
            classify_service("Dhcp", r"C:\Sample\dhcp.exe", Some("Sample Ltd")),
            StartupProtection::None
        );
    }

    #[test]
    fn tasks_are_classified_by_path_and_program() {
        assert_eq!(
            classify_task(r"\Microsoft\Windows\Foo\Bar", None, None),
            StartupProtection::Microsoft
        );
        assert_eq!(
            classify_task(r"\Microsoft\Windows\Windows Defender\Scan", None, None),
            StartupProtection::Critical
        );
        assert_eq!(
            classify_task(
                r"\SampleSync",
                Some(r"C:\Sample\sync.exe"),
                Some("Sample Ltd")
            ),
            StartupProtection::None
        );
        assert_eq!(
            classify_task(r"\Odd", Some(r"C:\Windows\System32\csrss.exe"), None),
            StartupProtection::Critical
        );
    }

    #[test]
    fn machine_wide_tasks_are_told_from_the_users_own() {
        for id in [
            "SYSTEM",
            r"NT AUTHORITY\SYSTEM",
            "S-1-5-18",
            "",
            r"NT AUTHORITY\LOCAL SERVICE",
        ] {
            assert!(task_is_machine_wide(id, false), "{id}");
        }
        assert!(task_is_machine_wide(r"PC\sample", true));
        assert!(!task_is_machine_wide(r"PC\sample", false));
    }
}
