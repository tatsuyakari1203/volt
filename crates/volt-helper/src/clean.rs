//! Machine-wide cleaning in the helper: scan and delete the rules of the helper's own catalogue
//! that need administrator rights, with the handle-based walker of [`volt_win::safetree`].
//!
//! A request names a rule id and, for a scan, a minimum age that can only raise the catalogue's; no
//! path, no pattern. The helper resolves the rule's root from the operating system itself, walks it
//! (see [`volt_win::safetree`] for what keeps the walk inside the root), and then:
//!
//! - **`clean_scan`** is read-only and answers counts, sizes and reasons, never a name. It keeps what
//!   it found (file identities, kinds, sizes, times) under a 128-bit random id that is good for one
//!   apply on this connection;
//! - **`clean_apply`** takes that id and deletes exactly what the scan found that is still the same
//!   and still old enough. Deletion is permanent. A rule whose folder a normal user can write to
//!   (the catalogue says so, or the scan found the root's access list lets a non-administrator
//!   write) is confirmed by a person on the helper's own surface every time, through the same
//!   [`ConfirmGate`] as an administrator tweak.
//!
//! Every walk runs on a worker thread with a deadline. A thread that cannot be stopped ends the
//! helper: the decision prefers an exit to a hung administrator process.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use volt_core::clean::machine::{
    MACHINE_DEADLINE_SECS, effective_min_age, machine_rule, needs_confirmation, pending_paths,
};
use volt_core::{
    CleanApplyReply, CleanScanReply, CleaningRule, CleaningRules, Clock, ConfirmAction,
    ConfirmGate, ConfirmRequest, Confirmer, ErrorCode, MachineProblem, PortError, Registry,
    RegistryHive, RegistryValue, RegistryValueRef, ReplyBody,
};
use volt_win::safetree::{self, Control, Limits, NoProbe, Policy, Stop, TreeError, TreeScan};

/// How many scans the helper keeps at once. A new scan of a rule replaces the older one of it.
pub const MAX_STORED_SCANS: usize = 4;

/// Port: where the root of a rule is on this machine.
pub trait MachineRoots: Send + Sync {
    /// The folder `rule` works in, resolved from the operating system.
    ///
    /// # Errors
    /// [`TreeError::RootRefused`] for a location the helper does not serve.
    fn root(&self, rule: &CleaningRule) -> Result<PathBuf, TreeError>;
}

/// The roots of the real helper: the Windows folder and `ProgramData`, from the system itself.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemRoots;

impl MachineRoots for SystemRoots {
    fn root(&self, rule: &CleaningRule) -> Result<PathBuf, TreeError> {
        safetree::machine_root(rule.root.location(), rule.root.relative())
    }
}

/// Port: the paths Windows has queued to rename or delete at the next restart.
pub trait PendingRenames: Send + Sync {
    /// The queued paths, lower case and plain (see [`pending_paths`]). An error means the list
    /// could not be read, and nothing is scanned or cleaned: what Windows still means to move
    /// must never be deleted by guess.
    fn paths(&self) -> Result<Vec<String>, PortError>;
}

/// The queue Windows keeps in `HKLM\SYSTEM\CurrentControlSet\Control\Session Manager`, value
/// `PendingFileRenameOperations`.
#[derive(Debug, Default, Clone, Copy)]
pub struct RegistryPending;

impl PendingRenames for RegistryPending {
    fn paths(&self) -> Result<Vec<String>, PortError> {
        let value = RegistryValueRef::new(
            RegistryHive::LocalMachine,
            r"SYSTEM\CurrentControlSet\Control\Session Manager",
            "PendingFileRenameOperations",
        );
        match volt_win::WinRegistry::new().read_value(&value)? {
            None => Ok(Vec::new()),
            Some(RegistryValue::MultiSz(entries)) => Ok(pending_paths(&entries)),
            Some(RegistryValue::Sz(entry)) => Ok(pending_paths(&[entry])),
            Some(_) => Err(PortError::InvalidInput(
                "the pending rename list is not a list of paths".to_owned(),
            )),
        }
    }
}

/// Port: 128 random bits as 32 lower-case hex digits, for scan ids.
pub trait IdSource: Send + Sync {
    /// A new id, or `None` when no randomness is available (then no scan is kept).
    fn next(&self) -> Option<String>;
}

/// Scan ids from the system's random number generator.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemIds;

impl IdSource for SystemIds {
    fn next(&self) -> Option<String> {
        volt_win::helper::args::random_hex_128().ok()
    }
}

struct Stored {
    id: String,
    rule_id: String,
    min_age_days: u32,
    confirm: bool,
    scan: TreeScan,
}

/// What the desk is built from.
pub struct CleanParts {
    /// The helper's own cleaning rules.
    pub rules: Arc<CleaningRules>,
    /// Where roots are.
    pub roots: Arc<dyn MachineRoots>,
    /// Source of the time ages are measured against.
    pub clock: Arc<dyn Clock>,
    /// Who is asked before a clean of a rule that needs it.
    pub confirmer: Arc<dyn Confirmer>,
    /// Scan ids.
    pub ids: Arc<dyn IdSource>,
    /// What Windows has queued for the next restart.
    pub pending: Arc<dyn PendingRenames>,
    /// The caps of every walk.
    pub limits: Limits,
    /// How long a walk may run before it is stopped.
    pub budget: Duration,
}

impl std::fmt::Debug for CleanParts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CleanParts").finish_non_exhaustive()
    }
}

impl CleanParts {
    /// The parts of the real helper around `rules` and the confirmer.
    #[must_use]
    pub fn product(rules: Arc<CleaningRules>, confirmer: Arc<dyn Confirmer>) -> Self {
        Self {
            rules,
            roots: Arc::new(SystemRoots),
            clock: Arc::new(volt_core::SystemClock),
            confirmer,
            ids: Arc::new(SystemIds),
            pending: Arc::new(RegistryPending),
            limits: Limits::default(),
            budget: Duration::from_secs(MACHINE_DEADLINE_SECS),
        }
    }
}

/// Serves the cleaning requests of one session.
pub struct CleanDesk {
    parts: CleanParts,
    gate: ConfirmGate,
    scans: Vec<Stored>,
}

impl std::fmt::Debug for CleanDesk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CleanDesk")
            .field("scans", &self.scans.len())
            .finish_non_exhaustive()
    }
}

fn problem_of(error: TreeError) -> MachineProblem {
    match error {
        TreeError::RootMissing => MachineProblem::RootMissing,
        TreeError::AccessDenied => MachineProblem::AccessDenied,
        TreeError::Unreadable => MachineProblem::Unreadable,
        _ => MachineProblem::RootRefused,
    }
}

/// A walk that cannot be stopped: the helper ends, which is the one thing that always works.
fn exit_stuck() -> ! {
    log::error!("a walk did not stop when it was cancelled: ending");
    std::process::exit(i32::from(crate::run::exit::STUCK));
}

impl CleanDesk {
    /// A desk with nothing scanned yet.
    #[must_use]
    pub fn new(parts: CleanParts) -> Self {
        Self {
            parts,
            gate: ConfirmGate::new(),
            scans: Vec::new(),
        }
    }

    fn policy(&self, rule: &CleaningRule, min_age_days: u32, pending: Vec<String>) -> Policy {
        Policy {
            pending_restart: pending,
            rule: rule.clone(),
            min_age_days,
            now: self.parts.clock.now(),
            // A rule that a person confirms every time may hold what a user planted; the person
            // knows. A silent rule's folders are meant to be ones only administrators can change.
            skip_user_writable: !rule.confirm_each_time,
        }
    }

    fn control(&self) -> Control {
        Control::new(Arc::new(AtomicBool::new(false)), self.parts.budget)
    }

    /// Scans `rule_id` (read-only).
    ///
    /// # Errors
    /// [`ErrorCode::UnknownRule`], [`ErrorCode::InvalidRequest`] (an age that is not plausible),
    /// [`ErrorCode::Internal`] (no random id).
    pub fn scan(
        &mut self,
        rule_id: &str,
        min_age_days: Option<u32>,
    ) -> Result<ReplyBody, ErrorCode> {
        let Some(rule) = machine_rule(&self.parts.rules, rule_id).cloned() else {
            return Err(ErrorCode::UnknownRule);
        };
        let Some(age) = effective_min_age(&rule, min_age_days) else {
            return Err(ErrorCode::InvalidRequest);
        };
        let problem = |problem: MachineProblem| {
            Ok(ReplyBody::CleanScan(CleanScanReply {
                scan: String::new(),
                rule: rule.id.clone(),
                files: 0,
                folders: 0,
                bytes: 0,
                min_age_days: age,
                truncated: false,
                confirm: rule.confirm_each_time,
                skipped: volt_core::ScanSkips::default(),
                problem: Some(problem),
            }))
        };
        let root = match self.parts.roots.root(&rule) {
            Ok(root) => root,
            Err(error) => return problem(problem_of(error)),
        };
        let Ok(pending) = self.parts.pending.paths() else {
            return problem(MachineProblem::Unreadable);
        };
        let policy = self.policy(&rule, age, pending);
        let control = self.control();
        let (limits, walk_control) = (self.parts.limits, control.clone());
        let scanned = safetree::run_bounded(&control, move || {
            safetree::scan(&root, &policy, &limits, &walk_control)
        })
        .unwrap_or_else(|_| exit_stuck());
        let scan = match scanned {
            Ok(scan) => scan,
            Err(error) => return problem(problem_of(error)),
        };
        let confirm = needs_confirmation(&rule, scan.root_writable_by_users());
        let reply = CleanScanReply {
            scan: String::new(),
            rule: rule.id.clone(),
            files: scan.files(),
            folders: scan.folders(),
            bytes: scan.bytes(),
            min_age_days: age,
            truncated: scan.truncated(),
            confirm,
            skipped: scan.skips(),
            problem: matches!(scan.stopped(), Some(Stop::TimedOut))
                .then_some(MachineProblem::TimedOut),
        };
        // A scan that found nothing, or that was stopped, has nothing to apply.
        if scan.stopped().is_some() || (scan.files() == 0 && scan.folders() == 0) {
            return Ok(ReplyBody::CleanScan(reply));
        }
        let Some(id) = self.parts.ids.next() else {
            return Err(ErrorCode::Internal);
        };
        self.scans.retain(|stored| stored.rule_id != rule.id);
        while self.scans.len() >= MAX_STORED_SCANS {
            self.scans.remove(0);
        }
        self.scans.push(Stored {
            id: id.clone(),
            rule_id: rule.id,
            min_age_days: age,
            confirm,
            scan,
        });
        Ok(ReplyBody::CleanScan(CleanScanReply { scan: id, ..reply }))
    }

    /// Deletes what the scan `scan_id` found, after the confirmation its rule needs.
    ///
    /// # Errors
    /// [`ErrorCode::UnknownScan`], [`ErrorCode::NotConfirmed`], [`ErrorCode::Blocked`],
    /// [`ErrorCode::CleanFailed`] (the folder could not be used any more). Nothing was deleted for
    /// any of them.
    pub fn apply(&mut self, scan_id: &str, now: Instant) -> Result<ReplyBody, ErrorCode> {
        // Single use: the scan leaves the store whatever happens next.
        let Some(position) = self.scans.iter().position(|stored| stored.id == scan_id) else {
            return Err(ErrorCode::UnknownScan);
        };
        let stored = self.scans.remove(position);
        let Some(rule) = machine_rule(&self.parts.rules, &stored.rule_id).cloned() else {
            return Err(ErrorCode::UnknownRule);
        };
        if stored.confirm {
            let request = ConfirmRequest {
                action: ConfirmAction::CleanRule,
                tweak: rule.id.clone(),
                title: rule.title.clone(),
                risk: rule.risk,
            };
            let decision = self.gate.decide(&*self.parts.confirmer, &request, now);
            if !decision.is_accepted() {
                return Err(if decision.is_blocked() {
                    ErrorCode::Blocked
                } else {
                    ErrorCode::NotConfirmed
                });
            }
        }
        let root = self
            .parts
            .roots
            .root(&rule)
            .map_err(|_| ErrorCode::CleanFailed)?;
        let pending = self
            .parts
            .pending
            .paths()
            .map_err(|_| ErrorCode::CleanFailed)?;
        let policy = self.policy(&rule, stored.min_age_days, pending);
        let control = self.control();
        let walk_control = control.clone();
        let scan = stored.scan;
        let result = safetree::run_bounded(&control, move || {
            safetree::apply(&root, &scan, &policy, &walk_control, &NoProbe)
        })
        .unwrap_or_else(|_| exit_stuck());
        let done = result.map_err(|error| {
            log::warn!("a machine clean could not use its folder: {error:?}");
            ErrorCode::CleanFailed
        })?;
        Ok(ReplyBody::CleanApply(CleanApplyReply {
            rule: rule.id,
            files: done.files,
            folders: done.folders,
            bytes: done.bytes,
            skipped: done.skipped,
            stopped: done.stopped.is_some(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use std::fs::{self, File};
    use std::io::Write as _;
    use std::sync::Mutex;
    use std::time::SystemTime;

    use volt_core::Verdict;
    use volt_core::elevation::fakes::FakeConfirmer;

    use super::*;

    const RULES: &str = r#"
[[rule]]
id = "machine-temp"
title = "Machine temp"
description = "d"
category = "temporary_files"
risk = "caution"
include = ["**"]
requires_admin = true
permanent = true

[rule.root]
location = "windows"
path = "Temp"

[rule.constraints]
min_age_days = 7
max_depth = 4
remove_empty_dirs = true

[[rule]]
id = "machine-always-asks"
title = "Machine always asks"
description = "d"
category = "temporary_files"
risk = "safe"
include = ["**"]
requires_admin = true
permanent = true
confirm_each_time = true

[rule.root]
location = "program_data"
path = "Ask"

[rule.constraints]
min_age_days = 7
max_depth = 4

[[rule]]
id = "user-rule"
title = "A user rule"
description = "d"
category = "temporary_files"
risk = "safe"
include = ["**"]

[rule.root]
location = "local_app_data"
path = "X"

[rule.constraints]
min_age_days = 1
max_depth = 2
"#;

    struct Roots(PathBuf);

    impl MachineRoots for Roots {
        fn root(&self, rule: &CleaningRule) -> Result<PathBuf, TreeError> {
            Ok(self.0.join(&rule.id))
        }
    }

    #[derive(Default)]
    struct Pending(Vec<String>);

    impl PendingRenames for Pending {
        fn paths(&self) -> Result<Vec<String>, PortError> {
            Ok(self.0.clone())
        }
    }

    #[derive(Default)]
    struct Ids(Mutex<u32>);

    impl IdSource for Ids {
        fn next(&self) -> Option<String> {
            let mut next = self.0.lock().expect("lock");
            *next += 1;
            Some(format!("{:032x}", *next))
        }
    }

    struct Rig {
        _dir: tempfile::TempDir,
        base: PathBuf,
        confirmer: Arc<FakeConfirmer>,
        desk: CleanDesk,
    }

    fn rig(confirmer: FakeConfirmer) -> Rig {
        rig_with_pending(confirmer, |_| Vec::new())
    }

    /// A rig whose Windows has queued the paths `queued` makes from the temp folder of the rig.
    fn rig_with_pending(confirmer: FakeConfirmer, queued: impl FnOnce(&str) -> Vec<String>) -> Rig {
        let dir = tempfile::tempdir().expect("temp");
        let canonical = dir.path().canonicalize().expect("canonical");
        let base = PathBuf::from(canonical.to_string_lossy().trim_start_matches(r"\\?\"));
        let confirmer = Arc::new(confirmer);
        let desk = CleanDesk::new(CleanParts {
            rules: Arc::new(CleaningRules::from_toml(RULES).expect("rules")),
            roots: Arc::new(Roots(base.clone())),
            clock: Arc::new(volt_core::SystemClock),
            confirmer: confirmer.clone(),
            ids: Arc::new(Ids::default()),
            pending: Arc::new(Pending(queued(&base.to_string_lossy().to_lowercase()))),
            limits: Limits::default(),
            budget: Duration::from_secs(30),
        });
        Rig {
            _dir: dir,
            base,
            confirmer,
            desk,
        }
    }

    fn old_file(path: &std::path::Path, size: usize) {
        fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
        let mut file = File::create(path).expect("create");
        file.write_all(&vec![b'x'; size]).expect("write");
        file.set_modified(SystemTime::now() - Duration::from_secs(40 * 86_400))
            .expect("age");
    }

    fn scan_reply(body: ReplyBody) -> CleanScanReply {
        match body {
            ReplyBody::CleanScan(reply) => reply,
            other => panic!("expected a scan reply, got {other:?}"),
        }
    }

    fn apply_reply(body: ReplyBody) -> CleanApplyReply {
        match body {
            ReplyBody::CleanApply(reply) => reply,
            other => panic!("expected an apply reply, got {other:?}"),
        }
    }

    #[test]
    fn only_machine_rules_are_served_and_an_age_can_only_be_raised() {
        let mut rig = rig(FakeConfirmer::accepting());
        assert_eq!(
            rig.desk.scan("user-rule", None).err(),
            Some(ErrorCode::UnknownRule)
        );
        assert_eq!(
            rig.desk.scan("nothing", None).err(),
            Some(ErrorCode::UnknownRule)
        );
        assert_eq!(
            rig.desk.scan("machine-temp", Some(99_999)).err(),
            Some(ErrorCode::InvalidRequest)
        );
        let root = rig.base.join("machine-temp");
        old_file(&root.join("a.tmp"), 10);
        let lower = scan_reply(rig.desk.scan("machine-temp", Some(1)).expect("scan"));
        assert_eq!(lower.min_age_days, 7, "the catalogue's age is a floor");
        assert_eq!(lower.files, 1);
        let higher = scan_reply(rig.desk.scan("machine-temp", Some(90)).expect("scan"));
        assert_eq!(higher.min_age_days, 90);
        assert_eq!(higher.files, 0, "the file is 40 days old");
    }

    #[test]
    fn a_scan_reports_counts_and_sizes_and_never_a_name() {
        let mut rig = rig(FakeConfirmer::accepting());
        let root = rig.base.join("machine-always-asks");
        old_file(&root.join("secret-name.tmp"), 100);
        old_file(&root.join("folder-name").join("inner-name.tmp"), 50);
        let reply = scan_reply(rig.desk.scan("machine-always-asks", None).expect("scan"));
        assert_eq!((reply.files, reply.bytes), (2, 150));
        assert_eq!(reply.scan.len(), 32);
        assert_eq!(reply.problem, None);
        assert!(reply.confirm, "this rule is always confirmed");
        let text = serde_json::to_string(&reply).expect("json");
        for name in ["secret-name", "folder-name", "inner-name"] {
            assert!(!text.contains(name), "{text}");
        }
        assert!(
            root.join("secret-name.tmp").exists(),
            "a scan deletes nothing"
        );
    }

    #[test]
    fn a_silent_rule_leaves_a_folder_a_user_can_write_to_and_counts_it() {
        let mut rig = rig(FakeConfirmer::accepting());
        let root = rig.base.join("machine-temp");
        old_file(&root.join("top.tmp"), 10);
        old_file(&root.join("users-folder").join("inner.tmp"), 10);
        let scan = scan_reply(rig.desk.scan("machine-temp", None).expect("scan"));
        assert_eq!(scan.files, 1, "only the file at the top");
        assert_eq!(scan.skipped.user_writable, 1);
        // The folder of this test is the user's own, so the root itself is one a user can write to.
        assert!(scan.confirm);
        let done = apply_reply(rig.desk.apply(&scan.scan, Instant::now()).expect("apply"));
        assert_eq!(done.files, 1);
        assert!(root.join("users-folder").join("inner.tmp").exists());
    }

    #[test]
    fn a_root_that_is_missing_or_empty_is_reported_and_keeps_no_scan() {
        let mut rig = rig(FakeConfirmer::accepting());
        let missing = scan_reply(rig.desk.scan("machine-temp", None).expect("scan"));
        assert_eq!(missing.problem, Some(MachineProblem::RootMissing));
        assert!(missing.scan.is_empty());
        fs::create_dir_all(rig.base.join("machine-temp")).expect("root");
        let empty = scan_reply(rig.desk.scan("machine-temp", None).expect("scan"));
        assert_eq!((empty.problem, empty.files), (None, 0));
        assert!(empty.scan.is_empty());
        assert_eq!(
            rig.desk.apply(&"0".repeat(32), Instant::now()).err(),
            Some(ErrorCode::UnknownScan)
        );
    }

    #[test]
    fn a_confirmed_apply_deletes_exactly_the_scan_and_the_scan_is_single_use() {
        let mut rig = rig(FakeConfirmer::accepting());
        let root = rig.base.join("machine-always-asks");
        old_file(&root.join("a.tmp"), 100);
        old_file(&root.join("sub").join("b.tmp"), 50);
        let scan = scan_reply(rig.desk.scan("machine-always-asks", None).expect("scan"));
        // Made after the scan: not in it, so not deleted.
        old_file(&root.join("later.tmp"), 7);

        let done = apply_reply(rig.desk.apply(&scan.scan, Instant::now()).expect("apply"));
        assert_eq!((done.files, done.bytes), (2, 150));
        assert_eq!(done.rule, "machine-always-asks");
        assert!(!root.join("a.tmp").exists() && !root.join("sub").join("b.tmp").exists());
        assert!(root.join("later.tmp").exists());
        // The person was asked, about this rule, in the catalogue's words.
        let asked = rig.confirmer.asked();
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].action, ConfirmAction::CleanRule);
        assert_eq!(asked[0].tweak, "machine-always-asks");
        assert_eq!(asked[0].title, "Machine always asks");
        // Single use.
        assert_eq!(
            rig.desk.apply(&scan.scan, Instant::now()).err(),
            Some(ErrorCode::UnknownScan)
        );
    }

    #[test]
    fn without_an_acceptance_nothing_is_deleted_and_a_denial_blocks_the_same_rule() {
        let mut rig = rig(FakeConfirmer::scripted(vec![
            Verdict::Denied,
            Verdict::Accepted,
        ]));
        let root = rig.base.join("machine-temp");
        old_file(&root.join("a.tmp"), 10);
        let start = Instant::now();
        let first = scan_reply(rig.desk.scan("machine-temp", None).expect("scan"));
        assert_eq!(
            rig.desk.apply(&first.scan, start).err(),
            Some(ErrorCode::NotConfirmed)
        );
        assert!(root.join("a.tmp").exists());
        let second = scan_reply(rig.desk.scan("machine-temp", None).expect("scan"));
        assert_eq!(
            rig.desk
                .apply(&second.scan, start + Duration::from_secs(60))
                .err(),
            Some(ErrorCode::Blocked)
        );
        assert_eq!(rig.confirmer.asked().len(), 1);
        assert!(root.join("a.tmp").exists());
    }

    #[test]
    fn nobody_to_ask_is_a_refusal_not_a_deletion() {
        let mut rig = rig(FakeConfirmer::unavailable());
        let root = rig.base.join("machine-always-asks");
        old_file(&root.join("a.tmp"), 10);
        let scan = scan_reply(rig.desk.scan("machine-always-asks", None).expect("scan"));
        assert!(
            scan.confirm,
            "the catalogue says so, whatever the folder allows"
        );
        assert_eq!(
            rig.desk.apply(&scan.scan, Instant::now()).err(),
            Some(ErrorCode::NotConfirmed)
        );
        assert!(root.join("a.tmp").exists());
    }

    #[test]
    fn a_newer_scan_of_a_rule_replaces_the_older_one() {
        let mut rig = rig(FakeConfirmer::accepting());
        let root = rig.base.join("machine-temp");
        old_file(&root.join("a.tmp"), 10);
        let first = scan_reply(rig.desk.scan("machine-temp", None).expect("scan"));
        let second = scan_reply(rig.desk.scan("machine-temp", None).expect("scan"));
        assert_ne!(first.scan, second.scan);
        assert_eq!(
            rig.desk.apply(&first.scan, Instant::now()).err(),
            Some(ErrorCode::UnknownScan)
        );
        assert!(rig.desk.apply(&second.scan, Instant::now()).is_ok());
    }

    #[test]
    fn an_item_that_changed_after_the_scan_is_reported_not_deleted() {
        let mut rig = rig(FakeConfirmer::accepting());
        let root = rig.base.join("machine-temp");
        old_file(&root.join("a.tmp"), 10);
        old_file(&root.join("b.tmp"), 10);
        let scan = scan_reply(rig.desk.scan("machine-temp", None).expect("scan"));
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(root.join("a.tmp"))
            .expect("open");
        file.write_all(b"more").expect("append");
        drop(file);
        let done = apply_reply(rig.desk.apply(&scan.scan, Instant::now()).expect("apply"));
        assert_eq!(done.files, 1);
        assert_eq!(done.skipped.changed, 1);
        assert!(root.join("a.tmp").exists());
    }

    #[test]
    fn a_file_windows_has_queued_for_a_restart_is_not_planned_or_deleted() {
        let mut rig = rig_with_pending(FakeConfirmer::accepting(), |base| {
            vec![format!("{base}\\machine-always-asks\\queued.tmp")]
        });
        let root = rig.base.join("machine-always-asks");
        old_file(&root.join("queued.tmp"), 10);
        old_file(&root.join("free.tmp"), 10);
        let scan = scan_reply(rig.desk.scan("machine-always-asks", None).expect("scan"));
        assert_eq!(scan.files, 1);
        assert_eq!(scan.skipped.pending_restart, 1);
        apply_reply(rig.desk.apply(&scan.scan, Instant::now()).expect("apply"));
        assert!(root.join("queued.tmp").exists());
        assert!(!root.join("free.tmp").exists());
    }

    #[test]
    fn a_pending_list_that_cannot_be_read_scans_nothing() {
        struct Broken;
        impl PendingRenames for Broken {
            fn paths(&self) -> Result<Vec<String>, PortError> {
                Err(PortError::AccessDenied("no".to_owned()))
            }
        }
        let mut rig = rig(FakeConfirmer::accepting());
        let root = rig.base.join("machine-always-asks");
        old_file(&root.join("a.tmp"), 10);
        rig.desk.parts.pending = Arc::new(Broken);
        let scan = scan_reply(rig.desk.scan("machine-always-asks", None).expect("scan"));
        assert_eq!(scan.problem, Some(MachineProblem::Unreadable));
        assert!(scan.scan.is_empty());
        assert!(root.join("a.tmp").exists());
    }

    #[test]
    fn scan_ids_are_random_hex_of_the_right_length() {
        let id = SystemIds.next().expect("randomness");
        assert_eq!(id.len(), 32);
        assert!(
            id.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        );
        assert_ne!(id, SystemIds.next().expect("randomness"));
    }
}
