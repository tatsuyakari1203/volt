//! The cases of the elevated CI job for the helper's own state and its administrator tweaks.
//!
//! Two phases, because the helper that accepts confirmations is a different build:
//!
//! - [`release_phase`] runs against the RELEASE helper (`run`): the state folder it made, the
//!   single-helper lock, a state folder a user can write to, and a tweak request that nobody can
//!   confirm (the release helper has no way to ask, so it refuses and changes nothing);
//! - [`tweaks_phase`] runs against the HARNESS-feature helper (`tweaks`), which the workflow swaps
//!   in for the release one after the proof that the release helper holds nothing of the harness:
//!   an interrupted apply settled at the start, and an apply and revert of the test-only tweak
//!   under a throwaway `HKLM\SOFTWARE\VoltTest` key with the harness's accepting confirmer.
//!
//! Everything here runs on a GitHub-hosted runner only (the harness refuses to start elsewhere).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use volt_core::{
    ActivityOutcome, Catalogue, ElevationError, Elevator, ErrorCode, Journal, NewEntry, Operation,
    Registry, RegistryHive, RegistryKeyRef, RegistryValue, RegistryValueRef, RegistryView, Request,
    Source, Timestamp, TweakStep,
};
use volt_helper::run::exit;
use volt_store::Store;
use volt_win::helper::acl::{self, Ace};
use volt_win::helper::args::{HelperArgs, PipeName};
use volt_win::helper::ci;
use volt_win::helper::identity::{ProcessFacts, ProcessHandle};
use volt_win::helper::install::{InstallLayout, check_security, is_trusted_principal, verify_tree};
use volt_win::helper::launch::HELPER_IMAGE_NAME;
use volt_win::helper::pipe::PipeClient;
use volt_win::{WinElevator, WinMemory, WinRegistry};

use super::{
    Report, exit_code_within, expect_exit, hello_over, icacls, millis, request_over, valid_args,
};

/// SID of SYSTEM.
const SYSTEM: &str = "S-1-5-18";
/// SID of the Administrators group.
const ADMINISTRATORS: &str = "S-1-5-32-544";
/// The built-in Users group, in the form `icacls` takes.
const USERS: &str = "*S-1-5-32-545";
/// The flag of an inherited ACE.
const INHERITED: u8 = 0x10;
/// The harness build's test-only tweak (see `ci/ci-tweak.toml`).
const CI_TWEAK: &str = "ci-voltest-marker";
/// The test-only tweak whose key the workflow made writable for Users.
const OPEN_KEY_TWEAK: &str = "ci-voltest-open";
/// The test-only tweak whose write ends the helper (the failpoint, see `src/failpoint.rs`).
const CRASH_TWEAK: &str = "ci-voltest-crash";
/// A real administrator tweak of the product catalogue that the refusal case asks for.
const REAL_TWEAK: &str = "turn-off-error-reporting";

fn state_dir(image: &Path) -> PathBuf {
    image.with_file_name("state")
}

fn describe(aces: &[Ace]) -> String {
    aces.iter()
        .map(|ace| format!("{}:{:#x}:{:#x}", ace.sid, ace.mask, ace.flags))
        .collect::<Vec<_>>()
        .join(", ")
}

// ---------------------------------------------------------------------------------------------
// Phase 1: the release helper

/// The cases that need no accepting confirmer.
pub(crate) fn release_phase(report: &mut Report, image: &Path, me: &ProcessFacts) {
    state_folder(report, image, me);
    single_helper(report, image, me);
    refused_without_confirmation(report);
    weak_state_folder(report, image, me);
}

/// What the helper left in the install folder: a state folder with a protected ACL, a folder and a
/// store for this user, the lock file.
fn state_folder(report: &mut Report, image: &Path, me: &ProcessFacts) {
    let dir = state_dir(image);
    let name = "the helper created its state folder with a protected ACL: SYSTEM and Administrators only, nothing inherited";
    match acl::of_path(&dir) {
        Ok(info) => {
            let aces = info.dacl.clone().unwrap_or_default();
            let only_administrators = !aces.is_empty()
                && aces
                    .iter()
                    .all(|ace| ace.sid == SYSTEM || ace.sid == ADMINISTRATORS);
            let inherits = aces.iter().any(|ace| ace.flags & INHERITED != 0);
            report.check(
                name,
                is_trusted_principal(&info.owner)
                    && only_administrators
                    && !inherits
                    && check_security(&info).is_ok(),
                format!("owner {}, entries [{}]", info.owner, describe(&aces)),
            );
        }
        Err(error) => report.check(name, false, error),
    }

    let user = dir.join(&me.token.user_sid);
    report.check(
        "the user's folder, the store and the lock file are in it",
        user.join("helper.db").is_file() && dir.join("helper.lock").is_file(),
        format!(
            "{} (store: {}, lock file: {})",
            user.display(),
            user.join("helper.db").is_file(),
            dir.join("helper.lock").is_file()
        ),
    );
    let user_name = "the user's folder is readable and writable by SYSTEM and Administrators only";
    match acl::of_path(&user) {
        Ok(info) => {
            let aces = info.dacl.clone().unwrap_or_default();
            report.check(
                user_name,
                is_trusted_principal(&info.owner)
                    && !aces.is_empty()
                    && aces
                        .iter()
                        .all(|ace| ace.sid == SYSTEM || ace.sid == ADMINISTRATORS),
                format!("owner {}, entries [{}]", info.owner, describe(&aces)),
            );
        }
        Err(error) => report.check(user_name, false, error),
    }
}

/// A helper that is running (it holds the lock, serves its pipe and has said hello).
struct Running {
    process: ProcessHandle,
    client: PipeClient,
}

fn start_helper(helper: &Path, me: &ProcessFacts) -> Result<Running, String> {
    let pipe = PipeName::random().map_err(|error| format!("{error:?}"))?;
    let args = HelperArgs {
        pipe: pipe.clone(),
        ui_pid: me.identity.pid,
        ui_created_at_100ns: me.identity.created_at_100ns,
    }
    .render();
    let process = ci::launch(helper, &args)?;
    let client = PipeClient::open_waiting(&pipe, Duration::from_secs(10))
        .map_err(|error| format!("the pipe did not open: {error:?}"))?;
    hello_over(&client)?;
    Ok(Running { process, client })
}

/// Says bye and returns the exit code of the helper.
fn stop_helper(running: &Running) -> Option<u32> {
    let _ = request_over(&running.client, 2, Request::Bye {});
    ci::wait_exit_code(&running.process, Duration::from_secs(20))
}

/// A second helper is refused while a first one runs, and a new one can run after the first ended.
fn single_helper(report: &mut Report, image: &Path, me: &ProcessFacts) {
    let helper = image.with_file_name(HELPER_IMAGE_NAME);
    let first = match start_helper(&helper, me) {
        Ok(first) => first,
        Err(error) => {
            report.check("a first helper starts and serves", false, error);
            return;
        }
    };
    report.check("a first helper starts and serves", true, "hello answered");

    // The second one has a valid command line, the bound UI and its own pipe name: only the lock
    // can stop it.
    let started = std::time::Instant::now();
    let second = exit_code_within(&helper, &valid_args(me), Duration::from_secs(40));
    report.check(
        "a second helper is refused while the first holds the lock (the lock code)",
        second == Ok(Some(u32::from(exit::LOCKED))),
        format!(
            "exit {second:?} after {}, expected {}",
            millis(started.elapsed()),
            exit::LOCKED
        ),
    );

    let code = stop_helper(&first);
    report.check(
        "the first helper was not disturbed and exits with code 0 after bye",
        code == Some(0),
        format!("{code:?}"),
    );
    drop(first);

    match start_helper(&helper, me) {
        Ok(third) => {
            let code = stop_helper(&third);
            report.check(
                "once the first helper has ended the lock is free again",
                code == Some(0),
                format!("{code:?}"),
            );
        }
        Err(error) => report.check(
            "once the first helper has ended the lock is free again",
            false,
            error,
        ),
    }
}

/// The values the registry steps of `tweak` read now, as text.
fn snapshot(registry: WinRegistry, tweak: &volt_core::Tweak) -> Vec<String> {
    tweak
        .steps
        .iter()
        .filter_map(|step| match step {
            TweakStep::RegistrySet {
                hive,
                subkey,
                name,
                view,
                ..
            }
            | TweakStep::RegistryDelete {
                hive,
                subkey,
                name,
                view,
                ..
            } => Some(format!(
                "{hive:?}\\{subkey}\\{name} = {:?}",
                registry.read_value(
                    &RegistryValueRef::new(*hive, subkey.clone(), name.clone()).with_view(*view)
                )
            )),
            _ => None,
        })
        .collect()
}

/// A tweak request that nobody answers: the release helper asks on its private desktop, nobody is
/// at the runner, so the question ends at its timeout. Nothing changes and the refusal is logged.
fn refused_without_confirmation(report: &mut Report) {
    let catalogue = match Catalogue::from_toml(volt_helper::embedded_tweak_catalogue()) {
        Ok(catalogue) => catalogue,
        Err(error) => {
            report.check("read the product catalogue", false, error.to_string());
            return;
        }
    };
    let Some(tweak) = catalogue.get(REAL_TWEAK) else {
        report.check(
            "find the administrator tweak of the case",
            false,
            REAL_TWEAK,
        );
        return;
    };
    let registry = WinRegistry::new();
    let before = snapshot(registry, tweak);

    // The release helper holds the product's catalogue.
    let elevator = WinElevator::new(volt_helper::product_catalogue_hashes());
    let began = std::time::Instant::now();
    let first = elevator.tweak_apply(REAL_TWEAK);
    let waited = began.elapsed();
    report.check(
        "an apply request that nobody answered is refused as not confirmed",
        first == Err(ElevationError::Refused(ErrorCode::NotConfirmed)),
        format!("{first:?} after {}", millis(waited)),
    );
    // A question that timed out is a denial, so the same request is blocked for ten minutes
    // without asking again. A runner that cannot show the prompt at all (no interactive desktop)
    // refuses the same way and counts no denial: asked again, it is refused again.
    let began = std::time::Instant::now();
    let second = elevator.tweak_apply(REAL_TWEAK);
    let quick = began.elapsed() < Duration::from_secs(10);
    let blocked = second == Err(ElevationError::Refused(ErrorCode::Blocked));
    let asked_again = second == Err(ElevationError::Refused(ErrorCode::NotConfirmed));
    report.check(
        "the same request is then blocked without asking (the unanswered question counted as a denial), or refused again when no prompt could be shown",
        (blocked && quick) || asked_again,
        format!("{second:?} after {}", millis(began.elapsed())),
    );
    report.fact(
        "the first question took",
        format!(
            "{} ({})",
            millis(waited),
            if blocked {
                "a prompt was shown and timed out"
            } else {
                "no prompt could be shown"
            }
        ),
    );
    let after = snapshot(registry, tweak);
    report.check(
        "the refused apply changed nothing in the registry",
        !before.is_empty() && before == after,
        format!("before {before:?}, after {after:?}"),
    );

    let revert = elevator.tweak_revert(REAL_TWEAK);
    report.check(
        "a revert of a change the helper never made has nothing to revert and asks nobody",
        revert == Err(ElevationError::Refused(ErrorCode::NothingToRevert)),
        format!("{revert:?}"),
    );
    let user_tweak = catalogue
        .tweaks()
        .iter()
        .find(|tweak| !tweak.requires_admin);
    let not_offered = [
        "no-such-tweak",
        user_tweak.map_or("show-file-extensions", |tweak| tweak.id.as_str()),
    ];
    for id in not_offered {
        let answer = elevator.tweak_apply(id);
        report.check(
            &format!("`{id}` is not an administrator tweak of the helper"),
            answer == Err(ElevationError::Refused(ErrorCode::UnknownTweak)),
            format!("{answer:?}"),
        );
    }

    refusals_in_the_log(report, &elevator);
    elevator.bye();
}

/// The refusals of the case above are in the helper's activity log, which the UI reads by pull.
fn refusals_in_the_log(report: &mut Report, elevator: &WinElevator) {
    let history = elevator.history(0, 20);
    match &history {
        Ok(reply) => {
            let refusals = reply
                .records
                .iter()
                .filter(|record| {
                    record.label.as_deref() == Some(REAL_TWEAK)
                        && record.outcome == ActivityOutcome::Failed
                        && record.summary.contains("Refused to apply")
                })
                .count();
            report.check(
                "both refusals are in the helper's activity log, which the UI can read by pull",
                refusals == 2 && reply.store.len() == 32,
                format!("{refusals} refusals, store {}", reply.store),
            );
        }
        Err(error) => report.check(
            "both refusals are in the helper's activity log, which the UI can read by pull",
            false,
            error.to_string(),
        ),
    }
}

/// A state folder that Users can write to: the helper refuses to run (the install code), and after
/// the entry is removed it runs again.
fn weak_state_folder(report: &mut Report, image: &Path, me: &ProcessFacts) {
    let helper = image.with_file_name(HELPER_IMAGE_NAME);
    let dir = state_dir(image);
    let dir_text = dir.to_string_lossy().into_owned();
    // Without inheritance flags: only the folder itself, so nothing inside it is touched.
    if let Err(error) = icacls(&[&dir_text, "/grant", &format!("{USERS}:M")]) {
        report.check("make the state folder writable for Users", false, error);
        return;
    }
    expect_exit(
        report,
        "a state folder that Users can write to is refused (the install code), nothing is served",
        &helper,
        &valid_args(me),
        exit::INSTALL,
    );
    let restored = icacls(&[&dir_text, "/remove:g", USERS]);
    let layout = InstallLayout::around(image);
    let verdict = layout.as_ref().map(verify_tree);
    report.check(
        "removing the entry makes the state folder sound again",
        restored.is_ok() && verdict.as_ref().is_some_and(Result::is_ok),
        format!("{restored:?}, {verdict:?}"),
    );
}

// ---------------------------------------------------------------------------------------------
// Phase 2: the harness-feature helper

fn marker() -> RegistryValueRef {
    RegistryValueRef::new(
        RegistryHive::LocalMachine,
        r"SOFTWARE\VoltTest\Ci",
        "Marker",
    )
}

/// Removes the throwaway keys, whatever is left of them.
fn clean_up(registry: WinRegistry, with_open_key: bool) {
    // The key the workflow made writable for Users is the workflow's: it is left alone at the start
    // and removed at the end.
    let open = with_open_key.then_some(r"SOFTWARE\VoltTest\Open");
    for subkey in [
        Some(r"SOFTWARE\VoltTest\Ci"),
        Some(r"SOFTWARE\VoltTest\Crash"),
        open,
        Some(r"SOFTWARE\VoltTest"),
    ]
    .into_iter()
    .flatten()
    {
        let _ = registry.delete_key(&RegistryKeyRef::new(
            RegistryHive::LocalMachine,
            subkey,
            RegistryView::Native,
        ));
    }
}

fn key_exists(registry: WinRegistry) -> bool {
    registry
        .key_exists(
            RegistryHive::LocalMachine,
            r"SOFTWARE\VoltTest\Ci",
            RegistryView::Native,
        )
        .unwrap_or(true)
}

/// An apply cut short by a kill: the helper's journal holds the pending entry (written before the
/// change) and the change itself was made. The helper is not running. This writes exactly that.
fn leave_an_interrupted_apply(
    report: &mut Report,
    image: &Path,
    me: &ProcessFacts,
    registry: WinRegistry,
) -> bool {
    let name = "an apply cut short is staged: a pending journal entry and the change itself";
    let database = state_dir(image).join(&me.token.user_sid).join("helper.db");
    if !database.is_file() {
        report.check(name, false, format!("no store at {}", database.display()));
        return false;
    }
    let catalogue = match Catalogue::from_toml(volt_helper::embedded_tweak_catalogue()) {
        Ok(catalogue) => Arc::new(catalogue),
        Err(error) => {
            report.check(name, false, error.to_string());
            return false;
        }
    };
    let ports = volt_helper::ports::helper_ports(&catalogue, Arc::new(WinMemory::new()));
    let operation = Operation::SetRegistryValue {
        target: marker(),
        value: RegistryValue::Dword(1),
    };
    let staged = (|| -> Result<(), String> {
        let prior = operation.detect(&ports).map_err(|e| e.to_string())?;
        let store = Store::open(&database).map_err(|e| e.to_string())?.store;
        let batch = store.new_batch(Some(CI_TWEAK)).map_err(|e| e.to_string())?;
        store
            .record_pending(NewEntry {
                batch,
                timestamp: Timestamp::from_unix_millis(0),
                source: Source::Manual,
                operation,
                prior,
            })
            .map_err(|e| e.to_string())?;
        registry
            .write_value(&marker(), &RegistryValue::Dword(1))
            .map_err(|e| e.to_string())
    })();
    report.check(name, staged.is_ok(), format!("{staged:?}"));
    staged.is_ok()
}

/// The cases against the harness-feature helper, which confirms everything.
pub(crate) fn tweaks_phase(report: &mut Report, image: &Path, me: &ProcessFacts) {
    let registry = WinRegistry::new();
    clean_up(registry, false);
    let staged = leave_an_interrupted_apply(report, image, me, registry);

    let elevator = WinElevator::new(volt_helper::catalogue_hashes());
    let hello = elevator.hello();
    report.check(
        "the harness helper starts and agrees on the catalogue",
        hello.is_ok(),
        format!("{:?}", hello.as_ref().map(|h| h.helper_version.clone())),
    );
    if hello.is_err() {
        clean_up(registry, true);
        return;
    }

    if staged {
        // The helper settled the pending entry when it started: the change had happened, so it is
        // an applied change the journal can revert, with the previous value (no key at all).
        let reverted = elevator.tweak_revert(CI_TWEAK);
        report.check(
            "the helper settled the interrupted apply at its start: the change is revertible",
            reverted.as_ref().is_ok_and(|reply| reply.complete),
            format!("{reverted:?}"),
        );
        report.check(
            "reverting it put the previous state back (no value, no key)",
            registry.read_value(&marker()) == Ok(None) && !key_exists(registry),
            format!("{:?}", registry.read_value(&marker())),
        );
    }

    let applied = elevator.tweak_apply(CI_TWEAK);
    report.check(
        "a confirmed apply of the test tweak is carried out",
        applied
            .as_ref()
            .is_ok_and(|reply| reply.outcome == volt_core::ApplyOutcome::Applied),
        format!("{applied:?}"),
    );
    report.check(
        "the value is in place under HKLM",
        registry.read_value(&marker()) == Ok(Some(RegistryValue::Dword(1))),
        format!("{:?}", registry.read_value(&marker())),
    );
    let again = elevator.tweak_apply(CI_TWEAK);
    report.check(
        "applying it again is a no-op",
        again
            .as_ref()
            .is_ok_and(|reply| reply.outcome == volt_core::ApplyOutcome::AlreadyApplied),
        format!("{again:?}"),
    );

    let history = elevator.history(0, 20);
    let last = history
        .as_ref()
        .ok()
        .and_then(|reply| reply.records.last().map(|record| record.id.0));
    report.check(
        "the history shows the apply, oldest first",
        history.as_ref().is_ok_and(|reply| {
            reply.records.iter().any(|record| {
                record.label.as_deref() == Some(CI_TWEAK)
                    && record.outcome == ActivityOutcome::Applied
            }) && reply.records.windows(2).all(|pair| pair[0].id < pair[1].id)
        }),
        format!("{:?}", history.as_ref().map(|reply| reply.records.len())),
    );
    let tail = elevator.history(last.unwrap_or(u64::MAX), 20);
    report.check(
        "reading after the last record gives nothing, so a mirror by cursor misses nothing",
        tail.as_ref()
            .is_ok_and(|reply| reply.records.is_empty() && !reply.more),
        format!("{:?}", tail.as_ref().map(|reply| reply.records.len())),
    );

    let reverted = elevator.tweak_revert(CI_TWEAK);
    report.check(
        "a confirmed revert restores the earlier state from the helper's journal",
        reverted.as_ref().is_ok_and(|reply| reply.complete)
            && registry.read_value(&marker()) == Ok(None)
            && !key_exists(registry),
        format!("{reverted:?}"),
    );
    let nothing = elevator.tweak_revert(CI_TWEAK);
    report.check(
        "a second revert has nothing to revert",
        nothing == Err(ElevationError::Refused(ErrorCode::NothingToRevert)),
        format!("{nothing:?}"),
    );

    open_key_refused(report, &elevator, registry);
    killed_mid_apply(report, &elevator, registry);

    elevator.bye();
    clean_up(registry, true);
}

/// A helper that really ends between the journal write and the change. The harness build's registry
/// ends the process at the first write of the crash tweak, after the engine journaled the previous
/// value. The request ends with no answer, nothing was changed, and the NEXT helper start settles
/// the pending entry: the activity log says the change never happened and there is nothing to
/// revert.
fn killed_mid_apply(report: &mut Report, elevator: &WinElevator, registry: WinRegistry) {
    let value = RegistryValueRef::new(
        RegistryHive::LocalMachine,
        r"SOFTWARE\VoltTest\Crash",
        "Marker",
    );
    let answer = elevator.tweak_apply(CRASH_TWEAK);
    report.check(
        "the helper ends at the failpoint, so the request gets no answer",
        matches!(
            answer,
            Err(ElevationError::Disconnected | ElevationError::TimedOut)
        ),
        format!("{answer:?}"),
    );
    report.check(
        "the change was never made: no value under the crash key",
        registry.read_value(&value) == Ok(None),
        format!("{:?}", registry.read_value(&value)),
    );

    // The next request starts a new helper, which settles the pending entry before it serves. The
    // old process may need a moment to let go of the single-helper lock.
    let mut hello = elevator.hello();
    for _ in 0..5 {
        if hello.is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
        hello = elevator.hello();
    }
    report.check(
        "the next helper starts and serves",
        hello.is_ok(),
        format!("{:?}", hello.as_ref().map(|h| h.helper_version.clone())),
    );
    // A reply holds as many records as fit one message, so read the log page by page to its end.
    let mut records = Vec::new();
    let mut after = 0_u64;
    for _ in 0..50 {
        let Ok(page) = elevator.history(after, 20) else {
            break;
        };
        let Some(last) = page.records.last().map(|record| record.id.0) else {
            break;
        };
        after = last;
        records.extend(page.records);
        if !page.more {
            break;
        }
    }
    let settled = records.iter().find(|record| {
        record
            .phrase
            .as_ref()
            .is_some_and(|phrase| phrase.code == "summary.interruptedNever")
    });
    report.check(
        "the next start settled the interrupted apply: the log says the change never happened",
        settled.is_some_and(|record| record.outcome == ActivityOutcome::Failed),
        format!(
            "{:?} of {} rows",
            settled.map(|record| record.summary.clone()),
            records.len()
        ),
    );
    let nothing = elevator.tweak_revert(CRASH_TWEAK);
    report.check(
        "an apply that never happened leaves nothing to revert",
        nothing == Err(ElevationError::Refused(ErrorCode::NothingToRevert)),
        format!("{nothing:?}"),
    );
}

/// A key on the path that a non-administrator can write: the workflow made `SOFTWARE\VoltTest\Open`
/// writable for Users. The confirmed apply must still change nothing, because the helper refuses to
/// write through that key.
fn open_key_refused(report: &mut Report, elevator: &WinElevator, registry: WinRegistry) {
    let open = RegistryValueRef::new(
        RegistryHive::LocalMachine,
        r"SOFTWARE\VoltTest\Open",
        "Marker",
    );
    let prepared = registry
        .key_exists(
            RegistryHive::LocalMachine,
            r"SOFTWARE\VoltTest\Open",
            RegistryView::Native,
        )
        .unwrap_or(false);
    report.check(
        "the key that Users can write to was prepared by the workflow",
        prepared,
        format!("{prepared}"),
    );
    let answer = elevator.tweak_apply(OPEN_KEY_TWEAK);
    report.check(
        "a confirmed apply through a key that Users can write to is refused, and rolled back",
        answer
            .as_ref()
            .is_ok_and(|reply| reply.outcome == volt_core::ApplyOutcome::RolledBack),
        format!("{answer:?}"),
    );
    report.check(
        "nothing was written below that key",
        registry.read_value(&open) == Ok(None),
        format!("{:?}", registry.read_value(&open)),
    );
    let history = elevator.history(0, 20);
    report.check(
        "the failed step is in the helper's activity log",
        history.as_ref().is_ok_and(|reply| {
            reply.records.iter().any(|record| {
                record.label.as_deref() == Some(OPEN_KEY_TWEAK)
                    && record.outcome == ActivityOutcome::Failed
            })
        }),
        format!("{:?}", history.as_ref().map(|reply| reply.records.len())),
    );
}
