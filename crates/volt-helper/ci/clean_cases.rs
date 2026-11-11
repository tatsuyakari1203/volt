//! The cases of the elevated CI job for machine-wide cleaning, against the HARNESS-feature helper.
//!
//! Everything works in throwaway folders the harness makes under `C:\ProgramData\VoltTest` of a
//! hosted runner (a virtual machine that is thrown away), through three test-only rules (see
//! `ci/ci-clean.toml`). The runner's real Windows Temp folder is only ever SCANNED (counts), never
//! cleaned. The cases are:
//!
//! - a scan and an apply on a folder only administrators can write to: no confirmation, exactly the
//!   old files go, a folder a user can write to, a junction and a new file are left alone;
//! - the confirmation: a rule the catalogue marks, and a rule whose folder the harness makes
//!   writable for Users (the helper finds that out from the access list itself);
//! - the race harness on the real elevated helper and a real NTFS volume: a junction swap, a hard
//!   link, a folder swapped for a file, a renamed ancestor and a REAL volume mount point, each
//!   done between the scan and the apply, each proving that nothing outside the root is deleted;
//! - the refusals: a rule that is not a machine rule, a scan id that was never made or already used.

use std::fs::{self, File, OpenOptions};
use std::io::Write as _;
use std::os::windows::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use volt_core::{
    CleanApplyReply, CleanScanReply, ElevationError, Elevator, ErrorCode, MachineProblem,
};
use volt_win::WinElevator;

use super::{Report, icacls};

const SILENT: &str = "ci-voltest-clean-silent";
const ASK: &str = "ci-voltest-clean-ask";
const OPEN: &str = "ci-voltest-clean-open";
/// SIDs in the form `icacls` takes.
const SYSTEM: &str = "*S-1-5-18";
const ADMINISTRATORS: &str = "*S-1-5-32-544";
const USERS: &str = "*S-1-5-32-545";

fn program_data() -> PathBuf {
    PathBuf::from(std::env::var("ProgramData").unwrap_or_else(|_| r"C:\ProgramData".to_owned()))
}

fn clean_base() -> PathBuf {
    program_data().join("VoltTest").join("Clean")
}

fn outside() -> PathBuf {
    program_data().join("VoltTest").join("Outside")
}

fn system_tool(name: &str) -> PathBuf {
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_owned());
    PathBuf::from(root).join("System32").join(name)
}

fn run_tool(name: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(system_tool(name))
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if output.status.success() {
        Ok(text)
    } else {
        Err(text)
    }
}

/// A file of `size` bytes last written `days` ago.
fn old_file(path: &Path, days: u64, size: usize) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut file = File::create(path).map_err(|e| e.to_string())?;
    file.write_all(&vec![b'x'; size])
        .map_err(|e| e.to_string())?;
    file.set_modified(SystemTime::now() - Duration::from_secs(days * 86_400))
        .map_err(|e| e.to_string())
}

/// A folder last written `days` ago.
fn old_dir(path: &Path, days: u64) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|e| e.to_string())?;
    let handle = OpenOptions::new()
        .write(true)
        // FILE_FLAG_BACKUP_SEMANTICS: a folder can be opened as a handle.
        .custom_flags(0x0200_0000)
        .open(path)
        .map_err(|e| e.to_string())?;
    handle
        .set_modified(SystemTime::now() - Duration::from_secs(days * 86_400))
        .map_err(|e| e.to_string())
}

fn junction(link: &Path, target: &Path) -> Result<(), String> {
    run_tool(
        "cmd.exe",
        &[
            "/C",
            "mklink",
            "/J",
            &link.to_string_lossy(),
            &target.to_string_lossy(),
        ],
    )
    .map(|_| ())
}

/// A fresh folder only SYSTEM and Administrators can change, made before anything is put in it so
/// that everything inside inherits that.
fn administrator_only(dir: &Path) -> Result<(), String> {
    let _ = fs::remove_dir_all(dir);
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let text = dir.to_string_lossy().into_owned();
    icacls(&[&text, "/inheritance:r"])?;
    icacls(&[&text, "/grant:r", &format!("{SYSTEM}:(OI)(CI)F")])?;
    icacls(&[&text, "/grant:r", &format!("{ADMINISTRATORS}:(OI)(CI)F")])
}

fn scan(elevator: &WinElevator, rule: &str) -> Result<CleanScanReply, ElevationError> {
    elevator.clean_scan(rule, None)
}

fn apply(elevator: &WinElevator, scan: &CleanScanReply) -> Result<CleanApplyReply, ElevationError> {
    elevator.clean_apply(&scan.scan)
}

fn tidy() {
    // A mount point must be unmounted before its folder is removed.
    unmount(&clean_base().join("Silent").join("m"));
    let _ = fs::remove_dir_all(program_data().join("VoltTest").join("Clean"));
    let _ = fs::remove_dir_all(outside());
}

pub(crate) fn clean_phase(report: &mut Report) {
    tidy();
    let elevator = WinElevator::new(volt_helper::catalogue_hashes());
    let hello = elevator.hello();
    report.check(
        "machine cleaning: the harness helper starts and agrees on both catalogues",
        hello.is_ok(),
        format!("{:?}", hello.as_ref().map(|h| h.helper_version.clone())),
    );
    if hello.is_err() {
        return;
    }
    silent_rule(report, &elevator);
    confirmed_rules(report, &elevator);
    races(report, &elevator);
    real_windows_temp_scan(report, &elevator);
    refusals(report, &elevator);
    elevator.bye();
    tidy();
}

// ---------------------------------------------------------------------------------------------

fn silent_rule(report: &mut Report, elevator: &WinElevator) {
    let root = clean_base().join("Silent");
    let outside = outside();
    let built = (|| -> Result<(), String> {
        let _ = fs::remove_dir_all(&outside);
        old_file(&outside.join("victim.tmp"), 30, 10)?;
        administrator_only(&root)?;
        old_file(&root.join("old1.tmp"), 30, 100)?;
        old_file(&root.join("new.tmp"), 0, 50)?;
        old_file(&root.join("sub").join("old2.tmp"), 30, 200)?;
        old_dir(&root.join("sub"), 30)?;
        old_dir(&root.join("empty"), 30)?;
        // A folder that Users may write to: left alone, with everything in it.
        old_file(&root.join("users-sub").join("inner.tmp"), 30, 10)?;
        icacls(&[
            &root.join("users-sub").to_string_lossy(),
            "/grant",
            &format!("{USERS}:(OI)(CI)M"),
        ])?;
        junction(&root.join("link"), &outside)
    })();
    if let Err(error) = built {
        report.check("machine cleaning: build the silent tree", false, error);
        return;
    }

    let scanned = scan(elevator, SILENT);
    let Ok(scanned) = scanned else {
        report.check(
            "machine cleaning: scan the silent rule",
            false,
            format!("{scanned:?}"),
        );
        return;
    };
    report.check(
        "a scan of a folder only administrators can write to needs no confirmation",
        !scanned.confirm && scanned.problem.is_none(),
        format!("{scanned:?}"),
    );
    report.check(
        "the scan counts the old files, the folders they empty and the sizes, and names nothing",
        scanned.files == 2
            && scanned.folders == 2
            && scanned.bytes == 300
            && scanned.min_age_days == 7
            && scanned.scan.len() == 32,
        format!("{scanned:?}"),
    );
    report.check(
        "the scan says what it left out: a new file, a folder Users can write to and a junction",
        scanned.skipped.too_new == 1
            && scanned.skipped.user_writable == 1
            && scanned.skipped.links == 1,
        format!("{:?}", scanned.skipped),
    );
    report.check(
        "a scan deletes nothing",
        root.join("old1.tmp").exists() && root.join("sub").join("old2.tmp").exists(),
        "old files are still there",
    );

    let applied = apply(elevator, &scanned);
    report.check(
        "an apply deletes exactly what the scan found",
        applied
            .as_ref()
            .is_ok_and(|done| done.files == 2 && done.folders == 2 && done.bytes == 300),
        format!("{applied:?}"),
    );
    report.check(
        "what it left: the new file, the folder Users can write to, the junction and what it points at",
        root.join("new.tmp").exists()
            && root.join("users-sub").join("inner.tmp").exists()
            && root.join("link").exists()
            && outside.join("victim.tmp").exists()
            && !root.join("old1.tmp").exists()
            && !root.join("sub").exists()
            && !root.join("empty").exists(),
        "checked on disk",
    );
    let again = apply(elevator, &scanned);
    report.check(
        "a scan id is good for one apply",
        again == Err(ElevationError::Refused(ErrorCode::UnknownScan)),
        format!("{again:?}"),
    );
}

fn confirmed_rules(report: &mut Report, elevator: &WinElevator) {
    // The catalogue marks this rule.
    let root = clean_base().join("Ask");
    let built = old_file(&root.join("old.tmp"), 30, 10);
    let scanned = scan(elevator, ASK);
    report.check(
        "a rule the catalogue marks is confirmed every time",
        built.is_ok() && scanned.as_ref().is_ok_and(|s| s.confirm && s.files == 1),
        format!("{scanned:?}"),
    );
    if let Ok(scanned) = &scanned {
        let applied = apply(elevator, scanned);
        report.check(
            "a confirmed clean deletes (the harness confirmer accepts)",
            applied.as_ref().is_ok_and(|done| done.files == 1) && !root.join("old.tmp").exists(),
            format!("{applied:?}"),
        );
    }

    // The catalogue does not mark this one: its folder is what makes the helper ask.
    let root = clean_base().join("Open");
    let built = (|| -> Result<(), String> {
        administrator_only(&root)?;
        icacls(&[
            &root.to_string_lossy(),
            "/grant",
            &format!("{USERS}:(OI)(CI)M"),
        ])?;
        old_file(&root.join("old.tmp"), 30, 10)
    })();
    let scanned = scan(elevator, OPEN);
    report.check(
        "a folder that Users can write to forces the confirmation, whatever the catalogue says",
        built.is_ok() && scanned.as_ref().is_ok_and(|s| s.confirm && s.files == 1),
        format!("{built:?} {scanned:?}"),
    );
    if let Ok(scanned) = &scanned {
        let applied = apply(elevator, scanned);
        report.check(
            "and the clean then deletes the old file",
            applied.as_ref().is_ok_and(|done| done.files == 1) && !root.join("old.tmp").exists(),
            format!("{applied:?}"),
        );
    }
}

// ---------------------------------------------------------------------------------------------
// The races, on the real helper and a real volume

/// What the race cases check afterwards.
struct Race<'a> {
    elevator: &'a WinElevator,
    root: PathBuf,
}

impl Race<'_> {
    /// Makes a clean Silent tree and the Outside folder (a file that must never be deleted).
    fn fresh(&self) -> Result<(), String> {
        unmount(&self.root.join("m"));
        let _ = fs::remove_dir_all(outside());
        old_file(&outside().join("victim.tmp"), 30, 10)?;
        old_file(&outside().join("sub").join("victim.tmp"), 30, 10)?;
        administrator_only(&self.root)
    }

    fn scan_apply(
        &self,
        between: impl FnOnce() -> Result<(), String>,
    ) -> Result<(CleanScanReply, CleanApplyReply), String> {
        let scanned = scan(self.elevator, SILENT).map_err(|e| format!("scan: {e:?}"))?;
        between()?;
        let applied = apply(self.elevator, &scanned).map_err(|e| format!("apply: {e:?}"))?;
        Ok((scanned, applied))
    }
}

/// Whether the files outside the root are all still there, untouched.
fn victims_intact() -> bool {
    [
        outside().join("victim.tmp"),
        outside().join("sub").join("victim.tmp"),
    ]
    .iter()
    .all(|path| fs::metadata(path).is_ok_and(|m| m.len() == 10))
}

fn races(report: &mut Report, elevator: &WinElevator) {
    let race = Race {
        elevator,
        root: clean_base().join("Silent"),
    };
    let root = &race.root;

    // Junction swap: the folder the scan went through becomes a junction to a folder outside the
    // root that holds a file of the same name.
    let result = race
        .fresh()
        .and_then(|()| old_file(&root.join("a").join("victim.tmp"), 30, 10))
        .and_then(|()| {
            race.scan_apply(|| {
                fs::remove_dir_all(root.join("a")).map_err(|e| e.to_string())?;
                junction(&root.join("a"), &outside())
            })
        });
    report.check(
        "race: a folder swapped for a junction after the scan is not entered, nothing outside is deleted",
        result.as_ref().is_ok_and(|(_, done)| done.files == 0 && done.skipped.links == 1)
            && victims_intact(),
        format!("{result:?}"),
    );

    // Hard link: the scanned file gets a second name after the scan.
    let result = race
        .fresh()
        .and_then(|()| old_file(&root.join("h.tmp"), 30, 10))
        .and_then(|()| {
            race.scan_apply(|| {
                fs::hard_link(root.join("h.tmp"), outside().join("alias.tmp"))
                    .map_err(|e| e.to_string())
            })
        });
    report.check(
        "race: a file that gains a second name after the scan is left alone, with both names",
        result
            .as_ref()
            .is_ok_and(|(_, done)| done.files == 0 && done.skipped.hard_links == 1)
            && root.join("h.tmp").exists()
            && outside().join("alias.tmp").exists(),
        format!("{result:?}"),
    );

    // Hard link, the other way: the scanned file is replaced by a hard link to an outside file.
    let result = race
        .fresh()
        .and_then(|()| old_file(&root.join("victim.tmp"), 30, 10))
        .and_then(|()| {
            race.scan_apply(|| {
                fs::remove_file(root.join("victim.tmp")).map_err(|e| e.to_string())?;
                fs::hard_link(outside().join("victim.tmp"), root.join("victim.tmp"))
                    .map_err(|e| e.to_string())
            })
        });
    report.check(
        "race: a file replaced by a hard link to an outside file is not deleted, the outside file keeps every name",
        result.as_ref().is_ok_and(|(_, done)| done.files == 0)
            && root.join("victim.tmp").exists()
            && victims_intact(),
        format!("{result:?}"),
    );

    // Directory-to-file swap.
    let result = race
        .fresh()
        .and_then(|()| old_dir(&root.join("d"), 30))
        .and_then(|()| {
            race.scan_apply(|| {
                fs::remove_dir(root.join("d")).map_err(|e| e.to_string())?;
                old_file(&root.join("d"), 30, 10)
            })
        });
    report.check(
        "race: a folder swapped for a file is not deleted",
        result
            .as_ref()
            .is_ok_and(|(_, done)| done.folders == 0 && done.skipped.changed == 1)
            && root.join("d").is_file(),
        format!("{result:?}"),
    );

    // Ancestor rename: the folder is renamed away and another one with the same names put in its
    // place.
    let result = race
        .fresh()
        .and_then(|()| old_file(&root.join("p").join("q.tmp"), 30, 10))
        .and_then(|()| {
            race.scan_apply(|| {
                fs::rename(root.join("p"), root.join("p-moved")).map_err(|e| e.to_string())?;
                old_file(&root.join("p").join("q.tmp"), 30, 10)
            })
        });
    report.check(
        "race: a folder renamed away and replaced is not taken for the scanned one, both files survive",
        result
            .as_ref()
            .is_ok_and(|(_, done)| done.files == 0 && done.skipped.changed == 1)
            && root.join("p").join("q.tmp").exists()
            && root.join("p-moved").join("q.tmp").exists(),
        format!("{result:?}"),
    );

    mount_point(report, &race);
}

/// The `\\?\Volume{...}\` name of the volume mounted as `D:`, when there is one.
fn second_volume() -> Option<String> {
    let text = run_tool("mountvol.exe", &["D:\\", "/L"]).ok()?;
    let name = text
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with(r"\\?\Volume{"))?;
    Some(name.to_owned())
}

fn unmount(folder: &Path) {
    if folder.exists() {
        let _ = run_tool("mountvol.exe", &[&folder.to_string_lossy(), "/D"]);
    }
}

/// A REAL volume mount point: a second volume mounted on a folder the scan planned to delete.
fn mount_point(report: &mut Report, race: &Race<'_>) {
    let name =
        "race: a volume mounted on a scanned folder after the scan is not entered or deleted";
    let Some(volume) = second_volume() else {
        report.check(
            name,
            true,
            "no second volume (D:) on this runner: the junction cases above cover the same reparse tag, \
             IO_REPARSE_TAG_MOUNT_POINT",
        );
        return;
    };
    let canary = PathBuf::from("D:\\").join("volt-ci-canary.tmp");
    let root = &race.root;
    let mounted = root.join("m");
    let result = race
        .fresh()
        .and_then(|()| old_file(&canary, 30, 10))
        .and_then(|()| old_dir(&mounted, 30))
        .and_then(|()| {
            race.scan_apply(|| {
                run_tool("mountvol.exe", &[&mounted.to_string_lossy(), &volume]).map(|_| ())
            })
        });
    let intact = fs::metadata(&canary).is_ok_and(|m| m.len() == 10);
    let still_mounted = canary.exists() && mounted.join("volt-ci-canary.tmp").exists();
    unmount(&mounted);
    let _ = fs::remove_file(&canary);
    report.check(
        name,
        result.as_ref().is_ok_and(|(scanned, done)| {
            scanned.folders == 1 && done.folders == 0 && done.skipped.total() == 1
        }) && intact
            && still_mounted,
        format!("{result:?}, canary intact: {intact}, still mounted: {still_mounted}"),
    );
}

// ---------------------------------------------------------------------------------------------

/// The runner's REAL Windows Temp folder, scanned only: counts, never a delete. A normal user can
/// create files there, so the helper must say it asks for a confirmation.
fn real_windows_temp_scan(report: &mut Report, elevator: &WinElevator) {
    let scanned = elevator.clean_scan("windows-temp", None);
    report.check(
        "the real Windows Temp is scanned (counts only) and a clean of it would be confirmed",
        scanned
            .as_ref()
            .is_ok_and(|s| s.confirm && s.problem.is_none_or(|p| p == MachineProblem::RootMissing)),
        match &scanned {
            Ok(s) => format!(
                "{} files, {} bytes, {:?} left out, confirm {}",
                s.files, s.bytes, s.skipped, s.confirm
            ),
            Err(error) => format!("{error:?}"),
        },
    );
}

fn refusals(report: &mut Report, elevator: &WinElevator) {
    let user_rule = elevator.clean_scan("user-temp", None);
    report.check(
        "a rule that is not a machine rule is refused",
        user_rule == Err(ElevationError::Refused(ErrorCode::UnknownRule)),
        format!("{user_rule:?}"),
    );
    let never = elevator.clean_apply(&"0".repeat(32));
    report.check(
        "a scan id that was never made is refused",
        never == Err(ElevationError::Refused(ErrorCode::UnknownScan)),
        format!("{never:?}"),
    );
    let lower = elevator.clean_scan("windows-temp", Some(1));
    report.check(
        "a request can raise the minimum age but never lower it",
        lower.as_ref().is_ok_and(|s| s.min_age_days == 7),
        format!("{:?}", lower.map(|s| s.min_age_days)),
    );
}
