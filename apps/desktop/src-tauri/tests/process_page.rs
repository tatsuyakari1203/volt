//! The Processes commands over a real core built on the in-memory fakes and a throwaway store.
//!
//! Nothing here touches this machine's processes: the port is `FakeProcesses`.

use tempfile::TempDir;
use volt_core::PortError;
use volt_core::fakes::FakeSystem;
use volt_desktop_lib::IpcError;
use volt_desktop_lib::catalogue::EMBEDDED_CATALOGUE;
use volt_desktop_lib::processes::{EndStatusView, EndabilityView, end_app_of, list_processes_of};
use volt_desktop_lib::wiring::{AppCore, build_core};

const MIB: u64 = 1 << 20;

fn core_with(system: &FakeSystem, dir: &TempDir) -> AppCore {
    build_core(
        system.ports(),
        &dir.path().join("volt.db"),
        EMBEDDED_CATALOGUE,
    )
    .expect("core builds")
}

#[test]
fn the_list_is_grouped_by_app_with_memory_counts_and_whether_each_may_be_ended() {
    let dir = TempDir::new().expect("temp dir");
    let system = FakeSystem::new();
    system.processes.add(200, "chrome.exe", 300 * MIB);
    system.processes.add(201, "Chrome.exe", 200 * MIB);
    system.processes.add(202, "explorer.exe", 90 * MIB);
    system.processes.add(600, "svchost.exe", 20 * MIB);
    system.processes.add(601, "svchost.exe", 20 * MIB);
    let core = core_with(&system, &dir);

    let list = list_processes_of(&core).expect("list");
    assert_eq!(list.process_count, 5);
    assert!(list.logical_cpus >= 1);
    let ids: Vec<_> = list.groups.iter().map(|g| g.id.as_str()).collect();
    assert_eq!(ids, ["chrome.exe", "explorer.exe", "svchost.exe"]);
    let chrome = &list.groups[0];
    assert_eq!((chrome.process_count, chrome.end_count), (2, 2));
    assert!((chrome.memory_bytes - 500.0 * 1_048_576.0).abs() < 1.0);
    assert_eq!(chrome.endability, EndabilityView::Allowed);
    // The first reading has nothing to compare CPU with.
    assert!(chrome.cpu_percent.is_none());
    assert_eq!(list.groups[1].endability, EndabilityView::Explorer);
    assert_eq!(list.groups[2].endability, EndabilityView::ProtectedWindows);
    assert_eq!(list.groups[2].end_count, 0);
    // The clock has millisecond resolution: let it tick, or there is no interval to measure.
    std::thread::sleep(std::time::Duration::from_millis(5));
    let second = list_processes_of(&core).expect("second");
    assert!(second.groups[0].cpu_percent.is_some());
}

#[test]
fn ending_an_app_ends_its_processes_reports_each_one_and_records_a_non_undoable_activity() {
    let dir = TempDir::new().expect("temp dir");
    let system = FakeSystem::new();
    for pid in 200..204 {
        system.processes.add(pid, "vendor.exe", MIB);
    }
    system.processes.add(300, "notepad.exe", MIB);
    system
        .processes
        .fail_terminate(202, PortError::AccessDenied("no".into()));
    system
        .processes
        .fail_terminate(203, PortError::NotFound("gone".into()));
    let core = core_with(&system, &dir);

    let result = end_app_of(&core, "vendor.exe").expect("end");
    assert_eq!(result.name, "vendor.exe");
    assert_eq!(
        (
            result.ended,
            result.needs_administrator,
            result.already_gone,
            result.failed
        ),
        (2, 1, 1, 0)
    );
    let status = |pid| {
        result
            .processes
            .iter()
            .find(|p| p.pid == pid)
            .expect("process")
            .status
    };
    assert_eq!(status(200), EndStatusView::Ended);
    assert_eq!(status(202), EndStatusView::NeedsAdministrator);
    assert_eq!(status(203), EndStatusView::AlreadyGone);
    assert!(result.recorded);

    // Only that app was touched.
    let left: Vec<u32> = volt_core::Processes::list(&*system.processes)
        .expect("list")
        .iter()
        .map(|p| p.pid)
        .collect();
    assert!(left.contains(&300) && left.contains(&202));
    assert!(!left.contains(&200));

    let records = core.activity.list(10, None).expect("activity");
    assert_eq!(records.len(), 1);
    assert!(records[0].summary.contains("cannot be undone"));
    assert!(records[0].summary.contains("vendor.exe"));
    assert_eq!((records[0].batch, records[0].entry), (None, None));
}

#[test]
fn windows_critical_apps_are_refused_by_the_command_whatever_the_ui_sent() {
    let dir = TempDir::new().expect("temp dir");
    let system = FakeSystem::new();
    system.processes.add(500, "winlogon.exe", MIB);
    system.processes.add(501, "lsass.exe", MIB);
    let core = core_with(&system, &dir);

    for id in ["winlogon.exe", "LSASS.EXE"] {
        assert!(matches!(
            end_app_of(&core, id),
            Err(IpcError::InvalidInput { .. })
        ));
    }
    assert!(matches!(
        end_app_of(&core, "nothing.exe"),
        Err(IpcError::NotFound { .. })
    ));
    assert_eq!(
        volt_core::Processes::list(&*system.processes)
            .expect("list")
            .len(),
        2
    );
    assert!(core.activity.list(10, None).expect("activity").is_empty());
}

#[test]
fn volt_itself_is_listed_as_protected_and_refused() {
    let dir = TempDir::new().expect("temp dir");
    let system = FakeSystem::new();
    let me = std::process::id();
    system.processes.add(me, "volt-desktop.exe", 80 * MIB);
    // The WebView is a child of the shell.
    system
        .processes
        .add_process(me + 1, me, "msedgewebview2.exe", 60 * MIB, 0);
    let core = core_with(&system, &dir);

    let list = list_processes_of(&core).expect("list");
    assert!(
        list.groups
            .iter()
            .all(|g| g.endability == EndabilityView::ProtectedVolt)
    );
    for id in ["volt-desktop.exe", "msedgewebview2.exe"] {
        assert!(matches!(
            end_app_of(&core, id),
            Err(IpcError::InvalidInput { .. })
        ));
    }
    assert_eq!(
        volt_core::Processes::list(&*system.processes)
            .expect("list")
            .len(),
        2
    );
}
