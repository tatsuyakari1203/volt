//! The cases of the elevated CI job for the helper's confirmation on a private desktop.
//!
//! A hosted runner has no person, so what these cases can show is what happens when NOBODY
//! answers, and that nothing outside the prompt can answer for a person:
//!
//! - the private desktop is created with an ACL for Administrators only and is gone after it is
//!   torn down;
//! - a tweak request whose question nobody answers ends at the timeout, changes nothing and is
//!   logged;
//! - that denial blocks the same request for the block time (the gate takes the time as an
//!   argument, so the cases move the clock instead of waiting ten minutes), and two denials lock
//!   every confirmed request for the session;
//! - a window of the prompt cannot be found from the default desktop, and keys sent from it do not
//!   answer the prompt.
//!
//! What they cannot show is a person pressing the accept button: that needs a real machine (see the
//! notes of the task). The harness has no way to press it, and the product helper has none.
//!
//! The cases run in this process with the REAL confirmer ([`DesktopConfirmer`]) over the harness's
//! test-only tweak and a throwaway store, with a short timeout so an unanswered question ends in
//! seconds.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use volt_core::{
    ActivityLog, ActivityOutcome, Catalogue, ConfirmRequest, Confirmer, DENIAL_BLOCK, ErrorCode,
    Journal, Registry, RegistryHive, RegistryValueRef, SystemClock, Verdict,
};
use volt_helper::confirm::DesktopConfirmer;
use volt_helper::ports::helper_ports;
use volt_helper::tweaks::{DeskParts, TweakDesk};
use volt_store::Store;
use volt_win::helper::ci;
use volt_win::helper::prompt::{self, PrivateDesktop, PromptAnswer, PromptText};
use volt_win::helper::restore::WinRestorePoints;
use volt_win::{WinMemory, WinRegistry};

use super::Report;

/// The harness build's test-only tweak (see `ci/ci-tweak.toml`).
const CI_TWEAK: &str = "ci-voltest-marker";
/// How long an unanswered question waits in these cases.
const SHORT: Duration = Duration::from_secs(4);

fn marker() -> RegistryValueRef {
    RegistryValueRef::new(
        RegistryHive::LocalMachine,
        r"SOFTWARE\VoltTest\Ci",
        "Marker",
    )
}

/// Asks through the real confirmer and remembers what happened.
struct Recording {
    inner: DesktopConfirmer,
    asked: AtomicU32,
    verdicts: Mutex<Vec<Verdict>>,
}

impl Confirmer for Recording {
    fn confirm(&self, request: &ConfirmRequest) -> Verdict {
        self.asked.fetch_add(1, Ordering::SeqCst);
        let verdict = self.inner.confirm(request);
        if let Ok(mut verdicts) = self.verdicts.lock() {
            verdicts.push(verdict);
        }
        verdict
    }
}

fn sample_text(seconds: u64) -> PromptText {
    PromptText {
        title: "Volt CI".to_owned(),
        heading: "Turn on \"CI marker\"?".to_owned(),
        body: "Nobody is at this machine: this question must end unanswered.".to_owned(),
        footer: format!("If nobody answers within {seconds} seconds, nothing is changed."),
        accept: "Turn on".to_owned(),
        cancel: "Cancel".to_owned(),
    }
}

/// Every case of this module.
pub(crate) fn prompt_phase(report: &mut Report) {
    private_desktop(report);
    unanswered_request(report);
    default_desktop_cannot_answer(report);
    restore_point_call(report);
}

/// The desktop exists while it is held, only Administrators can open it, and it is gone after.
fn private_desktop(report: &mut Report) {
    let desktop = match PrivateDesktop::create() {
        Ok(desktop) => desktop,
        Err(error) => {
            report.check("the private desktop is created", false, error);
            return;
        }
    };
    let name = desktop.name().to_owned();
    report.check(
        "the private desktop is created and can be found by its name",
        prompt::desktop_exists(&name),
        name.clone(),
    );
    match desktop.security_sddl() {
        Ok(sddl) => {
            // One allow entry, for Administrators: no other user, group or integrity level can open
            // it. (The UI's token holds Administrators as a deny-only group.)
            let allows = sddl.matches("(A;").count();
            report.check(
                "its ACL grants Administrators and nobody else",
                allows == 1 && sddl.contains(";;;BA)"),
                sddl,
            );
        }
        Err(error) => report.check("its ACL can be read", false, error),
    }
    drop(desktop);
    report.check(
        "after the teardown the desktop is gone",
        !prompt::desktop_exists(&name),
        name,
    );
    let input = prompt::input_desktop_name();
    report.fact("input desktop after the teardown", format!("{input:?}"));
}

/// A throwaway store and a desk over the real registry port, the real confirmer and the real
/// restore point port, for the test-only tweak.
fn desk(
    confirmer: Arc<Recording>,
    dir: &std::path::Path,
) -> Result<(TweakDesk, Arc<Store>), String> {
    let catalogue = Arc::new(
        Catalogue::from_toml(volt_helper::embedded_tweak_catalogue())
            .map_err(|error| error.to_string())?,
    );
    let store = Arc::new(
        Store::open(&dir.join("prompt.db"))
            .map_err(|error| error.to_string())?
            .store,
    );
    let ports = helper_ports(&catalogue, Arc::new(WinMemory::new()));
    let desk = TweakDesk::new(DeskParts {
        catalogue,
        ports,
        journal: store.clone(),
        activity: store.clone(),
        clock: Arc::new(SystemClock),
        confirmer,
        restore: Arc::new(WinRestorePoints::new()),
        store_id: "ci-prompt".to_owned(),
    })
    .map_err(|error| error.to_string())?;
    Ok((desk, store))
}

fn refusal_rows(store: &Store) -> usize {
    store
        .list(50, None)
        .map(|records| {
            records
                .iter()
                .filter(|record| {
                    record.label.as_deref() == Some(CI_TWEAK)
                        && record.outcome == ActivityOutcome::Failed
                        && record.summary.contains("Refused to apply")
                })
                .count()
        })
        .unwrap_or(0)
}

/// A request that nobody answers, the denial block and the lockout.
fn unanswered_request(report: &mut Report) {
    let dir = std::env::temp_dir().join(format!("volt-ci-prompt-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let confirmer = Arc::new(Recording {
        inner: DesktopConfirmer::with_timeout(SHORT),
        asked: AtomicU32::new(0),
        verdicts: Mutex::new(Vec::new()),
    });
    let (mut desk, store) = match desk(Arc::clone(&confirmer), &dir) {
        Ok(built) => built,
        Err(error) => {
            report.check("build a tweak desk with the real confirmer", false, error);
            return;
        }
    };
    let registry = WinRegistry::new();
    let before = registry.read_value(&marker());
    let start = Instant::now();

    let first = desk.apply(CI_TWEAK, start);
    let waited = start.elapsed();
    let verdicts = confirmer
        .verdicts
        .lock()
        .map(|verdicts| verdicts.clone())
        .unwrap_or_default();
    report.fact("verdicts of the real confirmer", format!("{verdicts:?}"));
    report.check(
        "a request nobody answers is refused as not confirmed",
        first == Err(ErrorCode::NotConfirmed),
        format!("{first:?} after {} ms", waited.as_millis()),
    );
    report.check(
        "it changed nothing: no value, nothing journaled",
        registry.read_value(&marker()) == before
            && store
                .entries_in_state(volt_core::EntryState::Pending)
                .map(|entries| entries.is_empty())
                .unwrap_or(false),
        format!("{:?}", registry.read_value(&marker())),
    );
    report.check(
        "the refusal is in the activity log",
        refusal_rows(&store) == 1,
        format!("{} rows", refusal_rows(&store)),
    );

    match verdicts.as_slice() {
        [Verdict::Denied] => {
            report.check(
                "it ended at the timeout, not at once (nobody pressed anything)",
                waited >= SHORT - Duration::from_millis(500),
                format!("{} ms of {} ms", waited.as_millis(), SHORT.as_millis()),
            );
            denial_rules(report, &mut desk, &confirmer, &store, start);
        }
        other => report.check(
            "no prompt could be shown here, so the request was refused without one (fail closed)",
            other == [Verdict::Unavailable],
            format!(
                "{other:?}: the denial rules need a shown prompt and are covered by the unit tests"
            ),
        ),
    }
    drop(desk);
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The same request is blocked for the block time without asking; the clock is moved, not waited.
fn denial_rules(
    report: &mut Report,
    desk: &mut TweakDesk,
    confirmer: &Recording,
    store: &Store,
    start: Instant,
) {
    let asked = confirmer.asked.load(Ordering::SeqCst);
    for (minutes, seconds) in [(0, 30), (5, 0), (9, 59)] {
        let at = start + Duration::from_secs(minutes * 60 + seconds);
        let began = Instant::now();
        let answer = desk.apply(CI_TWEAK, at);
        report.check(
            &format!("{minutes} min {seconds} s after the denial the same request is blocked"),
            answer == Err(ErrorCode::Blocked) && began.elapsed() < Duration::from_secs(2),
            format!("{answer:?} after {} ms", began.elapsed().as_millis()),
        );
    }
    report.check(
        "a blocked request shows no prompt",
        confirmer.asked.load(Ordering::SeqCst) == asked,
        format!(
            "{} questions asked in all",
            confirmer.asked.load(Ordering::SeqCst)
        ),
    );
    report.check(
        "the blocks are in the activity log too",
        refusal_rows(store) == 4,
        format!("{} rows", refusal_rows(store)),
    );

    // After the block time it is asked again, and nobody answers again.
    let later = start + DENIAL_BLOCK + Duration::from_secs(1);
    let began = Instant::now();
    let again = desk.apply(CI_TWEAK, later);
    report.check(
        "after the block time the request is asked again (and, unanswered, refused again)",
        again == Err(ErrorCode::NotConfirmed)
            && confirmer.asked.load(Ordering::SeqCst) == asked + 1
            && began.elapsed() >= SHORT - Duration::from_millis(500),
        format!("{again:?} after {} ms", began.elapsed().as_millis()),
    );

    // Two denials in the session: everything that needs a confirmation is refused without asking.
    let locked = desk.apply(CI_TWEAK, later + Duration::from_secs(3 * 60 * 60));
    report.check(
        "after two denials every confirmed request is refused for the rest of the session",
        locked == Err(ErrorCode::Blocked) && confirmer.asked.load(Ordering::SeqCst) == asked + 1,
        format!("{locked:?}"),
    );
}

/// While a prompt is up, a window of its class is not visible from the default desktop and keys
/// sent from the default desktop do not answer it.
fn default_desktop_cannot_answer(report: &mut Report) {
    let timeout = Duration::from_secs(7);
    let text = sample_text(timeout.as_secs());
    let began = Instant::now();
    let shown = std::thread::spawn(move || prompt::ask(&text, timeout));

    // Past the buttons' delay, so that an answer that did get through would count.
    std::thread::sleep(Duration::from_millis(2500));
    let visible = ci::prompt_window_visible_from_this_desktop();
    let sent = ci::send_answer_keys_from_this_desktop();
    let input = prompt::input_desktop_name();
    report.fact(
        "while the prompt was up",
        format!("input desktop {input:?}, {sent} key events accepted by SendInput"),
    );
    let answer = shown.join().unwrap_or(PromptAnswer::Unavailable(
        "the prompt thread panicked".to_owned(),
    ));
    let waited = began.elapsed();
    match answer {
        PromptAnswer::Unavailable(reason) => report.check(
            "no prompt could be shown here, so nothing was shown (fail closed)",
            true,
            reason,
        ),
        answer => {
            report.check(
                "a window of the prompt is not found from the default desktop",
                !visible,
                format!("visible from the default desktop: {visible}"),
            );
            report.check(
                "keys sent from the default desktop did not answer it: it ended unanswered, at the timeout",
                answer == PromptAnswer::TimedOut && waited >= timeout - Duration::from_millis(500),
                format!("{answer:?} after {} ms", waited.as_millis()),
            );
            report.check(
                "the user's desktop is the input desktop again afterwards",
                prompt::input_desktop_name().as_deref() == Ok("Default"),
                format!("{:?}", prompt::input_desktop_name()),
            );
        }
    }
}

/// The real restore point call, once, on this throwaway machine. What it shows depends on the
/// runner (System Protection is usually off there), so only that it returns, and what it said, is
/// recorded.
fn restore_point_call(report: &mut Report) {
    use volt_core::RestorePoints;

    let began = Instant::now();
    let result = WinRestorePoints::new().create("Volt CI: restore point probe");
    report.check(
        "the real restore point call returns, with a restore point or a reported failure",
        began.elapsed() < Duration::from_secs(90),
        format!("{result:?} after {} ms", began.elapsed().as_millis()),
    );
    report.fact("restore point call", format!("{result:?}"));
}
