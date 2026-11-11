//! Tests of the winget update listing and installing, over synthetic output and a fake winget.

use std::sync::Arc;

use super::fakes::FakeWinget;
use super::*;
use crate::fakes::{FakeActivityLog, ManualClock};

const TABLE: &str = "\
Name              Id                Version   Available  Source
-------------------------------------------------------------------
Example Editor    Acme.Editor       1.2.3     1.3.0      winget
Sample Tool 2     Contoso.Tool2     < 4.0     4.1.2      winget
Demo Player       9NTESTPLAYER1     2.0.0     2.1.0      msstore
3 upgrades available.
";

#[test]
fn a_table_is_read_from_the_right_whatever_the_names_hold() {
    let parsed = parse_upgrade_list(TABLE).unwrap();
    let rows: Vec<_> = parsed
        .updates
        .iter()
        .map(|u| {
            (
                u.name.as_str(),
                u.id.as_str(),
                u.current.as_str(),
                u.available.as_str(),
            )
        })
        .collect();
    assert_eq!(
        rows,
        [
            ("Example Editor", "Acme.Editor", "1.2.3", "1.3.0"),
            ("Sample Tool 2", "Contoso.Tool2", "<4.0", "4.1.2"),
            ("Demo Player", "9NTESTPLAYER1", "2.0.0", "2.1.0"),
        ]
    );
    assert_eq!(parsed.updates[2].source.as_deref(), Some("msstore"));
    assert_eq!(parsed.skipped, 0);
}

#[test]
fn headers_in_another_language_and_wide_names_do_not_matter() {
    let text = "\
Tên ứng dụng      Mã            Phiên bản  Có sẵn   Nguồn
------------------------------------------------------------
编辑器 示例        Acme.Editor   1.0        1.1      winget
";
    let parsed = parse_upgrade_list(text).unwrap();
    assert_eq!(parsed.updates.len(), 1);
    assert_eq!(parsed.updates[0].name, "编辑器 示例");
    assert_eq!(parsed.updates[0].id, "Acme.Editor");
}

#[test]
fn progress_lines_and_escape_sequences_are_ignored() {
    let text = "   - \r   \\ \r\u{1b}[2K\n  |\nName  Id  Version  Available\n-----------------------------\nApp One  Acme.One  1.0  2.0\n";
    let parsed = parse_upgrade_list(text).unwrap();
    assert_eq!(parsed.updates.len(), 1);
    assert_eq!(parsed.updates[0].source, None);
}

#[test]
fn a_row_cut_short_is_counted_and_never_guessed() {
    let text = "\
Name  Id  Version  Available  Source
----------------------------------------------
Good App  Acme.Good  1.0  2.0  winget
Cut App  Acme.Very.Long.Pack…  1.0  2.0  winget
Other  Acme.Other  1.0  2…  winget
";
    let parsed = parse_upgrade_list(text).unwrap();
    assert_eq!(parsed.updates.len(), 1);
    assert_eq!(parsed.skipped, 2);
}

#[test]
fn the_second_table_and_the_footer_are_not_read() {
    let text = "\
Name  Id  Version  Available  Source
----------------------------------------------
App One  Acme.One  1.0  2.0  winget

The following packages have an upgrade available, but require explicit targeting for upgrade:
Name  Id  Version  Available  Source
----------------------------------------------
Pinned  Acme.Pinned  1.0  2.0  winget
";
    let parsed = parse_upgrade_list(text).unwrap();
    assert_eq!(parsed.updates.len(), 1);
}

#[test]
fn output_without_a_table_is_said_to_have_none() {
    assert_eq!(parse_upgrade_list("hello\nworld"), Err(NoTable::Missing));
    let narrow = "A  B  C\n---------\nx  y  z\n";
    assert_eq!(parse_upgrade_list(narrow), Err(NoTable::Columns));
}

#[test]
fn package_ids_and_sources_are_validated_before_they_become_arguments() {
    for good in ["Acme.Editor", "9NBLGGH4NNS1", "A+B_c-1.2"] {
        assert!(PackageId::parse(good).is_some(), "{good}");
    }
    for bad in [
        "",
        "-x",
        "a b",
        "a;b",
        "a&b",
        "a\"b",
        "a|b",
        "%PATH%",
        "--accept-source-agreements",
    ] {
        assert!(PackageId::parse(bad).is_none(), "{bad}");
    }
    assert!(PackageSource::parse("winget").is_some());
    assert!(PackageSource::parse("Win Get").is_none());
    assert!(PackageSource::parse("-x").is_none());
}

#[test]
fn the_argument_lists_are_fixed_and_never_accept_an_agreement() {
    assert_eq!(
        WingetCommand::List.args(),
        ["upgrade", "--disable-interactivity"]
    );
    let command = WingetCommand::Upgrade {
        id: PackageId::parse("Acme.Editor").unwrap(),
        source: PackageSource::parse("winget"),
    };
    assert_eq!(
        command.args(),
        [
            "upgrade",
            "--id",
            "Acme.Editor",
            "--exact",
            "--source",
            "winget",
            "--disable-interactivity"
        ]
    );
    assert!(!command.args().iter().any(|arg| arg.contains("accept")));
}

struct Fixture {
    winget: Arc<FakeWinget>,
    activity: Arc<FakeActivityLog>,
    manager: UpdateManager,
}

fn fixture() -> Fixture {
    let winget = Arc::new(FakeWinget::new());
    let activity = Arc::new(FakeActivityLog::new());
    let manager = UpdateManager::new(winget.clone(), Arc::new(ManualClock::new(1_000)))
        .with_activity_log(activity.clone());
    Fixture {
        winget,
        activity,
        manager,
    }
}

#[test]
fn a_missing_winget_is_reported_with_a_fix() {
    let f = fixture();
    let problem = f.manager.check().unwrap_err();
    assert_eq!(problem, UpdateProblem::Missing);
    assert!(!problem.fix().english().is_empty());
}

#[test]
fn a_winget_that_cannot_start_is_broken() {
    let f = fixture();
    f.winget.list_fails(PortError::AccessDenied("x".into()));
    assert_eq!(
        f.manager.check(),
        Err(UpdateProblem::Broken("access denied"))
    );
}

#[test]
fn source_agreements_are_reported_and_never_accepted() {
    let f = fixture();
    f.winget.lists(
        -1_978_335_212,
        "The source requires that you view the following agreements before using.\nDo you agree to all the source agreements terms?\n[Y] Yes  [N] No:",
    );
    assert_eq!(f.manager.check(), Err(UpdateProblem::Agreements));
    assert!(
        f.winget
            .commands()
            .iter()
            .all(|command| !command.args().iter().any(|arg| arg.contains("accept")))
    );
}

#[test]
fn nothing_to_update_is_an_empty_list_and_anything_else_is_not_guessed() {
    let f = fixture();
    f.winget
        .lists(0, "No installed package found matching input criteria.");
    assert_eq!(f.manager.check().unwrap(), UpdateListing::default());
    f.winget.lists(1, "Something went wrong.");
    assert_eq!(f.manager.check(), Err(UpdateProblem::Failed(1)));
    f.winget.lists(
        0,
        "one line\ntwo lines\nthree lines\nfour lines\nfive lines",
    );
    assert_eq!(f.manager.check(), Err(UpdateProblem::Unrecognised));
}

#[test]
fn an_update_is_installed_by_exact_id_and_recorded_as_not_undoable() {
    let f = fixture();
    f.winget.lists(0, TABLE);
    f.manager.check().unwrap();
    let report = f.manager.upgrade("Acme.Editor", Source::Manual).unwrap();
    assert_eq!(report.outcome, UpgradeOutcome::Updated);
    assert_eq!(
        (report.from.as_str(), report.to.as_str()),
        ("1.2.3", "1.3.0")
    );
    assert!(!report.activity_write_failed);
    let commands = f.winget.commands();
    assert_eq!(
        commands[1],
        WingetCommand::Upgrade {
            id: PackageId::parse("Acme.Editor").unwrap(),
            source: PackageSource::parse("winget"),
        }
    );
    let records = f.activity.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].outcome, ActivityOutcome::Applied);
    assert!(records[0].summary.contains("cannot be undone by Volt"));
    // Updated: no longer offered.
    assert_eq!(
        f.manager.upgrade("Acme.Editor", Source::Manual),
        Err(UpdateError::NotFound)
    );
}

#[test]
fn restart_failure_and_unstartable_outcomes_are_told_apart() {
    let f = fixture();
    f.winget.lists(0, TABLE);
    f.manager.check().unwrap();
    f.winget.upgrade_exits("Acme.Editor", 3010);
    f.winget.upgrade_exits("Contoso.Tool2", -1_978_335_189);
    f.winget
        .upgrade_fails("9NTESTPLAYER1", PortError::TimedOut("x".into()));
    assert_eq!(
        f.manager
            .upgrade("Acme.Editor", Source::Manual)
            .unwrap()
            .outcome,
        UpgradeOutcome::NeedsRestart
    );
    let failed = f.manager.upgrade("Contoso.Tool2", Source::Manual).unwrap();
    assert_eq!(
        failed.outcome,
        UpgradeOutcome::Failed(UpgradeFailure::Code(-1_978_335_189))
    );
    assert_eq!(
        f.manager
            .upgrade("9NTESTPLAYER1", Source::Manual)
            .unwrap()
            .outcome,
        UpgradeOutcome::Failed(UpgradeFailure::TimedOut)
    );
    let records = f.activity.records();
    assert_eq!(records[1].outcome, ActivityOutcome::Failed);
    assert!(records[1].summary.contains("0x8a15002b"));
}

#[test]
fn only_ids_of_the_last_check_can_be_installed() {
    let f = fixture();
    assert_eq!(
        f.manager.upgrade("Acme.Editor", Source::Manual),
        Err(UpdateError::NotFound)
    );
    f.winget.lists(0, TABLE);
    f.manager.check().unwrap();
    assert_eq!(
        f.manager
            .upgrade("--accept-source-agreements", Source::Manual),
        Err(UpdateError::NotFound)
    );
    assert_eq!(f.winget.commands().len(), 1);
}
