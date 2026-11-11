//! Administrator tweaks in the helper: the tweak engine over the helper's own journal, behind the
//! helper's own confirmation.
//!
//! A request names a tweak id and nothing else. The helper looks the id up in its OWN embedded
//! catalogue (the UI's copy plays no part), serves only tweaks that require administrator rights,
//! and takes every step, value and service from that catalogue. Then, for a change that persists:
//!
//! 1. a request that would change nothing (already applied, nothing to revert) is answered without
//!    asking anyone and writes nothing;
//! 2. the [`ConfirmGate`] applies the denial rules and asks the [`Confirmer`]. Without an
//!    acceptance nothing changes and the refusal is written to the helper's activity log;
//! 3. the engine detects, writes the previous value to the helper's journal and only then changes
//!    anything ([`TweakEngine`] through the executor), so an apply that is cut short is resolved
//!    by [`TweakDesk::new`] the next time the helper starts.
//!
//! The desk keeps no state about the system: the journal is the source of truth.

use std::sync::Arc;
use std::time::Instant;

use volt_core::{
    ActivityId, ActivityLog, ActivityOutcome, Catalogue, CatalogueIssue, Clock, ConfirmAction,
    ConfirmGate, ConfirmRequest, Confirmer, Decision, ErrorCode, HISTORY_MAX_RECORDS, HistoryReply,
    Journal, MAX_REPLY_BYTES, NewActivity, Phrase, PortError, Ports, ReconcileReport, Reply,
    ReplyBody, RestoreGate, RestorePoints, Source, Tweak, TweakApplyReply, TweakEngine, TweakError,
    TweakRevertReply, TweakState, needs_restore_point,
};

/// Why a desk cannot be built.
#[derive(Debug)]
pub enum DeskError {
    /// A tweak that requires administrator rights has a step the helper must never run.
    Catalogue(Vec<CatalogueIssue>),
    /// The journal could not be read to resolve an interrupted apply.
    Journal(PortError),
}

impl std::fmt::Display for DeskError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Catalogue(issues) => {
                write!(f, "the catalogue is not fit for the helper:")?;
                for issue in issues {
                    write!(f, " {issue};")?;
                }
                Ok(())
            }
            Self::Journal(error) => write!(f, "the journal cannot be read: {error}"),
        }
    }
}

impl std::error::Error for DeskError {}

/// Everything the desk is built from.
pub struct DeskParts {
    /// The helper's own catalogue.
    pub catalogue: Arc<Catalogue>,
    /// The ports the engine works through (a guarded registry, allow-listed services).
    pub ports: Ports,
    /// The helper's journal.
    pub journal: Arc<dyn Journal>,
    /// The helper's activity log.
    pub activity: Arc<dyn ActivityLog>,
    /// The clock.
    pub clock: Arc<dyn Clock>,
    /// Who is asked before a change.
    pub confirmer: Arc<dyn Confirmer>,
    /// Makes the restore point a Caution or Advanced tweak needs.
    pub restore: Arc<dyn RestorePoints>,
    /// The identifier of the helper's store, reported with the history.
    pub store_id: String,
}

impl std::fmt::Debug for DeskParts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeskParts")
            .field("store_id", &self.store_id)
            .finish_non_exhaustive()
    }
}

/// Serves the administrator-tweak requests of one session.
pub struct TweakDesk {
    engine: TweakEngine,
    catalogue: Arc<Catalogue>,
    activity: Arc<dyn ActivityLog>,
    clock: Arc<dyn Clock>,
    confirmer: Arc<dyn Confirmer>,
    gate: ConfirmGate,
    restore: Arc<dyn RestorePoints>,
    restore_gate: RestoreGate,
    store_id: String,
    recovered: ReconcileReport,
}

impl std::fmt::Debug for TweakDesk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TweakDesk")
            .field("store_id", &self.store_id)
            .finish_non_exhaustive()
    }
}

impl TweakDesk {
    /// Builds the desk and resolves what an earlier helper left in flight: an apply that was cut
    /// short has journal entries still pending, and each is settled by reading the system (see
    /// [`volt_core::Executor::reconcile_pending`]). This runs before the first request, so nothing
    /// can be applied or reverted on top of an unresolved change.
    ///
    /// # Errors
    /// [`DeskError::Catalogue`] when the catalogue breaks the administrator rules,
    /// [`DeskError::Journal`] when the journal cannot be read.
    pub fn new(parts: DeskParts) -> Result<Self, DeskError> {
        let issues = parts.catalogue.administrator_issues();
        if !issues.is_empty() {
            return Err(DeskError::Catalogue(issues));
        }
        let engine = TweakEngine::new(
            Arc::clone(&parts.catalogue),
            parts.ports,
            parts.journal,
            Arc::clone(&parts.clock),
        )
        .with_administrator(true)
        .with_activity_log(Arc::clone(&parts.activity));
        let recovered = engine
            .executor()
            .reconcile_pending()
            .map_err(DeskError::Journal)?;
        Ok(Self {
            engine,
            catalogue: parts.catalogue,
            activity: parts.activity,
            clock: parts.clock,
            confirmer: parts.confirmer,
            gate: ConfirmGate::new(),
            restore: parts.restore,
            restore_gate: RestoreGate::new(),
            store_id: parts.store_id,
            recovered,
        })
    }

    /// What was resolved when the desk was built.
    #[must_use]
    pub fn recovered(&self) -> &ReconcileReport {
        &self.recovered
    }

    /// The tweak with this id when the helper offers it: it is in the catalogue and requires
    /// administrator rights.
    fn offered(&self, id: &str) -> Option<&Tweak> {
        self.catalogue.get(id).filter(|tweak| tweak.requires_admin)
    }

    /// Applies the tweak after a confirmation.
    ///
    /// # Errors
    /// The code the session answers with; nothing was changed for any of them.
    pub fn apply(&mut self, id: &str, now: Instant) -> Result<ReplyBody, ErrorCode> {
        let Some(tweak) = self.offered(id) else {
            return Err(ErrorCode::UnknownTweak);
        };
        let (title, risk) = (tweak.title.clone(), tweak.risk);
        match self.engine.state(id) {
            // Nothing to do: nothing is asked and nothing is written.
            Ok(TweakState::Applied) => {
                return Ok(ReplyBody::TweakApply(TweakApplyReply {
                    tweak: id.to_owned(),
                    outcome: volt_core::ApplyOutcome::AlreadyApplied,
                }));
            }
            Ok(_) => {}
            Err(error) => return Err(self.failed(id, "apply", &error)),
        }
        self.confirm(ConfirmAction::ApplyTweak, id, title.clone(), risk, now)?;
        if needs_restore_point(risk) {
            self.restore_point(id, &title, now)?;
        }
        match self.engine.apply(id, Source::Manual) {
            Ok(report) => Ok(ReplyBody::TweakApply(TweakApplyReply {
                tweak: id.to_owned(),
                outcome: report.outcome,
            })),
            Err(error) => Err(self.failed(id, "apply", &error)),
        }
    }

    /// Reverts what the helper's journal holds for the tweak, after a confirmation.
    ///
    /// # Errors
    /// The code the session answers with; nothing was changed for any of them.
    pub fn revert(&mut self, id: &str, now: Instant) -> Result<ReplyBody, ErrorCode> {
        let Some(tweak) = self.offered(id) else {
            return Err(ErrorCode::UnknownTweak);
        };
        let (title, risk) = (tweak.title.clone(), tweak.risk);
        match self.engine.applied_by_volt(id) {
            Ok(true) => {}
            // Nothing the journal can restore: nothing is asked and nothing is written.
            Ok(false) => return Err(ErrorCode::NothingToRevert),
            Err(error) => return Err(self.failed(id, "revert", &error)),
        }
        self.confirm(ConfirmAction::RevertTweak, id, title, risk, now)?;
        match self.engine.revert_as(id, Source::Manual) {
            Ok(report) => Ok(ReplyBody::TweakRevert(TweakRevertReply {
                tweak: id.to_owned(),
                batches: u32::try_from(report.batches.len()).unwrap_or(u32::MAX),
                complete: report.is_complete(),
            })),
            Err(TweakError::NothingToRevert(_)) => Err(ErrorCode::NothingToRevert),
            Err(error) => Err(self.failed(id, "revert", &error)),
        }
    }

    /// The activity log after the record `after`, oldest first, as many records as fit one reply
    /// (at most `limit` and at most [`HISTORY_MAX_RECORDS`]).
    ///
    /// # Errors
    /// [`ErrorCode::Internal`] when the log cannot be read.
    pub fn history(&self, after: u64, limit: u32) -> Result<ReplyBody, ErrorCode> {
        let wanted = usize::try_from(limit.min(HISTORY_MAX_RECORDS)).unwrap_or(0);
        let mut records = self
            .activity
            .list_after(Some(ActivityId(after)), wanted + 1)
            .map_err(|error| {
                log::warn!("the activity log cannot be read: {error}");
                ErrorCode::Internal
            })?;
        let mut more = records.len() > wanted;
        records.truncate(wanted);
        // A reply is bounded: send fewer records rather than none.
        loop {
            let reply = HistoryReply {
                store: self.store_id.clone(),
                records: records.clone(),
                more,
            };
            let size = serde_json::to_vec(&Reply {
                id: u64::MAX,
                body: ReplyBody::History(reply.clone()),
            })
            .map_or(usize::MAX, |bytes| bytes.len());
            if size <= MAX_REPLY_BYTES || records.len() <= 1 {
                return Ok(ReplyBody::History(reply));
            }
            records.pop();
            more = true;
        }
    }

    /// Makes the restore point a Caution or Advanced tweak needs before it changes anything (one
    /// per ten minutes of a session). When Windows cannot make one the tweak is refused and the
    /// refusal is logged: the requirement is the helper's, so it is never skipped silently.
    fn restore_point(&mut self, id: &str, title: &str, now: Instant) -> Result<(), ErrorCode> {
        let description = format!("Volt: {title}");
        match self.restore_gate.ensure(&*self.restore, &description, now) {
            Ok(_) => Ok(()),
            Err(error) => {
                // Only the class of the failure: a port's text can carry data.
                log::warn!(
                    "no restore point for `{id}`: {}",
                    volt_core::activity::error_class(&error)
                );
                self.write(
                    id,
                    Phrase::new("summary.adminRefused")
                        .phrase("verb", Phrase::new("verb.apply"))
                        .text("tweak", id)
                        .phrase("reason", Phrase::new("reason.restorePoint")),
                );
                Err(ErrorCode::RestorePointFailed)
            }
        }
    }

    /// Asks (or refuses without asking) and logs a refusal.
    fn confirm(
        &mut self,
        action: ConfirmAction,
        id: &str,
        title: String,
        risk: volt_core::Risk,
        now: Instant,
    ) -> Result<(), ErrorCode> {
        let request = ConfirmRequest {
            action,
            tweak: id.to_owned(),
            title,
            risk,
        };
        let decision = self.gate.decide(&*self.confirmer, &request, now);
        if decision.is_accepted() {
            return Ok(());
        }
        let verb = match action {
            ConfirmAction::ApplyTweak => "verb.apply",
            _ => "verb.revert",
        };
        self.note_refusal(id, verb, decision);
        Err(if decision.is_blocked() {
            ErrorCode::Blocked
        } else {
            ErrorCode::NotConfirmed
        })
    }

    /// Writes the refusal of a change that nobody confirmed. A log that cannot take the record is
    /// reported to the diagnostic log; it never lets the change through.
    fn note_refusal(&self, id: &str, verb: &str, decision: Decision) {
        self.write(
            id,
            Phrase::new("summary.adminRefused")
                .phrase("verb", Phrase::new(verb))
                .text("tweak", id)
                .phrase("reason", decision.reason_phrase()),
        );
    }

    /// Logs an engine failure and gives the code to answer with.
    fn failed(&self, id: &str, verb: &str, error: &TweakError) -> ErrorCode {
        // Only the class of the failure: a port's text can carry data.
        log::warn!("administrator tweak `{id}` ({verb}): {error}");
        self.write(
            id,
            Phrase::new("summary.adminFailed").text("tweak", id).phrase(
                "verb",
                Phrase::new(if verb == "apply" {
                    "verb.apply"
                } else {
                    "verb.revert"
                }),
            ),
        );
        ErrorCode::TweakFailed
    }

    fn write(&self, id: &str, phrase: Phrase) {
        let record = NewActivity::said(
            self.clock.now(),
            Source::Manual,
            ActivityOutcome::Failed,
            phrase,
            Some(id.to_owned()),
        );
        if let Err(error) = self.activity.record(record) {
            log::warn!("the activity log could not take a record: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use volt_core::elevation::fakes::{FakeConfirmer, FakeRestorePoints};
    use volt_core::fakes::{FakeActivityLog, FakeSystem};
    use volt_core::{
        ApplyOutcome, DENIAL_BLOCK, EntryState, RegistryHive, RegistryValue, RegistryValueRef,
        Verdict,
    };

    use super::*;

    const CATALOGUE: &str = r#"
[[tweak]]
id = "machine-flag"
title = "Machine flag"
description = "Sets a machine-wide flag."
category = "privacy"
risk = "caution"
evidence = "measured"
evidence_note = "Test."
requires_admin = true

[[tweak.steps]]
kind = "registry_set"
hive = "HKLM"
subkey = 'SOFTWARE\VoltTest\Desk'
name = "Flag"
value = { dword = 1 }

[[tweak]]
id = "user-flag"
title = "User flag"
description = "Not an administrator tweak."
category = "privacy"
risk = "safe"
evidence = "measured"
evidence_note = "Test."

[[tweak.steps]]
kind = "registry_set"
hive = "HKCU"
subkey = 'Software\VoltTest\Desk'
name = "Flag"
value = { dword = 1 }
"#;

    fn flag() -> RegistryValueRef {
        RegistryValueRef::new(
            RegistryHive::LocalMachine,
            r"SOFTWARE\VoltTest\Desk",
            "Flag",
        )
    }

    struct Rig {
        system: FakeSystem,
        confirmer: Arc<FakeConfirmer>,
        restore: Arc<FakeRestorePoints>,
        desk: TweakDesk,
        start: Instant,
    }

    fn rig(confirmer: FakeConfirmer) -> Rig {
        rig_with(confirmer, FakeRestorePoints::new())
    }

    fn rig_with(confirmer: FakeConfirmer, restore: FakeRestorePoints) -> Rig {
        let system = FakeSystem::new();
        let confirmer = Arc::new(confirmer);
        let restore = Arc::new(restore);
        let desk = TweakDesk::new(DeskParts {
            catalogue: Arc::new(Catalogue::from_toml(CATALOGUE).expect("catalogue")),
            ports: system.ports(),
            journal: system.journal.clone(),
            activity: system.activity.clone(),
            clock: system.clock.clone(),
            confirmer: confirmer.clone(),
            restore: restore.clone(),
            store_id: "store-1".to_owned(),
        })
        .expect("desk");
        Rig {
            system,
            confirmer,
            restore,
            desk,
            start: Instant::now(),
        }
    }

    fn outcome(body: ReplyBody) -> ApplyOutcome {
        match body {
            ReplyBody::TweakApply(reply) => reply.outcome,
            other => panic!("expected an apply reply, got {other:?}"),
        }
    }

    #[test]
    fn a_confirmed_apply_journals_the_previous_value_first_and_a_confirmed_revert_restores_it() {
        let mut rig = rig(FakeConfirmer::accepting());
        rig.system.registry.set(&flag(), RegistryValue::Dword(7));

        let applied = rig.desk.apply("machine-flag", rig.start).expect("applied");
        assert_eq!(outcome(applied), ApplyOutcome::Applied);
        assert_eq!(
            rig.system.registry.get(&flag()),
            Some(RegistryValue::Dword(1))
        );
        // The person was asked once, about this tweak, in the catalogue's own words.
        let asked = rig.confirmer.asked();
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].tweak, "machine-flag");
        assert_eq!(asked[0].title, "Machine flag");
        assert_eq!(asked[0].risk, volt_core::Risk::Caution);
        // The journal holds the previous value.
        let entries = rig
            .system
            .journal
            .entries_in_state(EntryState::Applied)
            .expect("entries");
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].prior,
            volt_core::PriorState::RegistryValue(Some(RegistryValue::Dword(7)))
        );

        let reverted = rig
            .desk
            .revert("machine-flag", rig.start)
            .expect("reverted");
        let ReplyBody::TweakRevert(reply) = reverted else {
            panic!("expected a revert reply");
        };
        assert!(reply.complete);
        assert_eq!(reply.batches, 1);
        assert_eq!(
            rig.system.registry.get(&flag()),
            Some(RegistryValue::Dword(7))
        );
        assert_eq!(rig.confirmer.asked().len(), 2);
    }

    #[test]
    fn without_an_acceptance_nothing_changes_and_the_refusal_is_logged() {
        for confirmer in [FakeConfirmer::unavailable(), FakeConfirmer::denying()] {
            let mut rig = rig(confirmer);
            let refused = rig.desk.apply("machine-flag", rig.start);
            assert_eq!(refused, Err(ErrorCode::NotConfirmed));
            assert_eq!(rig.system.registry.get(&flag()), None, "nothing changed");
            assert!(
                rig.system
                    .journal
                    .entries_in_state(EntryState::Pending)
                    .expect("entries")
                    .is_empty(),
                "nothing was even journaled"
            );
            let records = rig.system.activity.records();
            assert_eq!(records.len(), 1, "{records:?}");
            assert_eq!(records[0].outcome, ActivityOutcome::Failed);
            assert_eq!(records[0].label.as_deref(), Some("machine-flag"));
            assert!(records[0].summary.contains("Refused to apply"));
        }
    }

    #[test]
    fn a_denial_blocks_the_same_request_for_ten_minutes_and_two_denials_lock_the_session() {
        let mut rig = rig(FakeConfirmer::scripted(vec![
            Verdict::Denied,
            Verdict::Accepted,
        ]));
        assert_eq!(
            rig.desk.apply("machine-flag", rig.start),
            Err(ErrorCode::NotConfirmed)
        );
        // Blocked without asking, up to the tenth minute.
        for seconds in [1, 120, 599] {
            assert_eq!(
                rig.desk
                    .apply("machine-flag", rig.start + Duration::from_secs(seconds)),
                Err(ErrorCode::Blocked),
                "{seconds} s"
            );
        }
        assert_eq!(rig.confirmer.asked().len(), 1);
        assert_eq!(rig.system.registry.get(&flag()), None);
        // Each refusal is in the log.
        assert_eq!(rig.system.activity.records().len(), 4);
        // After ten minutes it is asked again, and this time accepted.
        let applied = rig
            .desk
            .apply("machine-flag", rig.start + DENIAL_BLOCK)
            .expect("applied");
        assert_eq!(outcome(applied), ApplyOutcome::Applied);
    }

    #[test]
    fn two_denials_refuse_every_confirmed_request_for_the_rest_of_the_session() {
        let mut rig = rig(FakeConfirmer::denying());
        assert_eq!(
            rig.desk.apply("machine-flag", rig.start),
            Err(ErrorCode::NotConfirmed)
        );
        // The revert of a tweak that was never applied has nothing to do, so nothing is asked for
        // it; the second denial comes from an apply after the first block ran out.
        let later = rig.start + DENIAL_BLOCK;
        assert_eq!(
            rig.desk.apply("machine-flag", later),
            Err(ErrorCode::NotConfirmed)
        );
        assert_eq!(
            rig.desk
                .apply("machine-flag", later + Duration::from_secs(3 * 3600)),
            Err(ErrorCode::Blocked)
        );
        assert_eq!(rig.confirmer.asked().len(), 2);
    }

    #[test]
    fn a_request_that_would_change_nothing_asks_nobody_and_writes_nothing() {
        let mut rig = rig(FakeConfirmer::accepting());
        rig.system.registry.set(&flag(), RegistryValue::Dword(1));
        let already = rig.desk.apply("machine-flag", rig.start).expect("answered");
        assert_eq!(outcome(already), ApplyOutcome::AlreadyApplied);
        assert_eq!(
            rig.desk.revert("machine-flag", rig.start),
            Err(ErrorCode::NothingToRevert),
            "a change made outside Volt has no journal entry to restore"
        );
        assert!(rig.confirmer.asked().is_empty());
        assert!(rig.system.activity.records().is_empty());
        assert!(
            rig.system
                .journal
                .entries_in_state(EntryState::Applied)
                .expect("entries")
                .is_empty()
        );
    }

    #[test]
    fn only_administrator_tweaks_of_the_catalogue_are_offered() {
        let mut rig = rig(FakeConfirmer::accepting());
        for id in ["user-flag", "no-such-tweak", "", "../x", "MACHINE-FLAG"] {
            assert_eq!(
                rig.desk.apply(id, rig.start),
                Err(ErrorCode::UnknownTweak),
                "{id:?}"
            );
            assert_eq!(
                rig.desk.revert(id, rig.start),
                Err(ErrorCode::UnknownTweak),
                "{id:?}"
            );
        }
        assert!(rig.confirmer.asked().is_empty());
        assert!(rig.system.activity.records().is_empty());
    }

    #[test]
    fn a_failing_step_is_rolled_back_logged_and_reported_as_a_failure_to_carry_out() {
        let mut rig = rig(FakeConfirmer::accepting());
        rig.system
            .registry
            .fail_on(&flag(), PortError::AccessDenied("refused".to_owned()));
        let result = rig.desk.apply("machine-flag", rig.start).expect("answered");
        assert_eq!(outcome(result), ApplyOutcome::RolledBack);
        assert_eq!(rig.system.registry.get(&flag()), None);
    }

    #[test]
    fn a_catalogue_with_a_command_or_a_per_user_step_in_an_administrator_tweak_is_refused() {
        let system = FakeSystem::new();
        let bad = CATALOGUE.replace("hive = \"HKLM\"", "hive = \"HKCU\"");
        let built = TweakDesk::new(DeskParts {
            catalogue: Arc::new(Catalogue::from_toml(&bad).expect("catalogue")),
            ports: system.ports(),
            journal: system.journal.clone(),
            activity: system.activity.clone(),
            clock: system.clock.clone(),
            confirmer: Arc::new(FakeConfirmer::accepting()),
            restore: Arc::new(volt_core::elevation::fakes::FakeRestorePoints::new()),
            store_id: "s".to_owned(),
        });
        assert!(matches!(built, Err(DeskError::Catalogue(_))));
    }

    #[test]
    fn the_history_is_read_oldest_first_by_cursor_and_says_whether_there_is_more() {
        let mut rig = rig(FakeConfirmer::denying());
        // Records with the refusals of three different requests.
        for _ in 0..1 {
            let _ = rig.desk.apply("machine-flag", rig.start);
        }
        for seconds in [1, 2] {
            let _ = rig
                .desk
                .apply("machine-flag", rig.start + Duration::from_secs(seconds));
        }
        assert_eq!(rig.system.activity.records().len(), 3);

        let ReplyBody::History(first) = rig.desk.history(0, 2).expect("history") else {
            panic!("expected a history reply");
        };
        assert_eq!(first.store, "store-1");
        assert_eq!(
            first.records.iter().map(|r| r.id.0).collect::<Vec<_>>(),
            [1, 2]
        );
        assert!(first.more);
        let ReplyBody::History(second) = rig.desk.history(2, 2).expect("history") else {
            panic!("expected a history reply");
        };
        assert_eq!(
            second.records.iter().map(|r| r.id.0).collect::<Vec<_>>(),
            [3]
        );
        assert!(!second.more);
        let ReplyBody::History(none) = rig.desk.history(3, 2).expect("history") else {
            panic!("expected a history reply");
        };
        assert!(none.records.is_empty() && !none.more);
    }

    #[test]
    fn a_history_reply_never_exceeds_the_reply_limit() {
        let rig = rig(FakeConfirmer::accepting());
        for index in 0..HISTORY_MAX_RECORDS {
            rig.system
                .activity
                .record(NewActivity {
                    timestamp: volt_core::Timestamp::from_unix_millis(i64::from(index)),
                    source: Source::Manual,
                    outcome: ActivityOutcome::Applied,
                    summary: "x".repeat(400),
                    batch: None,
                    entry: None,
                    label: None,
                    phrase: None,
                })
                .expect("record");
        }
        let ReplyBody::History(reply) = rig.desk.history(0, HISTORY_MAX_RECORDS).expect("history")
        else {
            panic!("expected a history reply");
        };
        assert!(!reply.records.is_empty());
        assert!(reply.records.len() < HISTORY_MAX_RECORDS as usize);
        assert!(reply.more);
        let bytes = volt_core::encode_reply(&Reply {
            id: 9,
            body: ReplyBody::History(reply),
        });
        assert!(bytes.len() <= MAX_REPLY_BYTES);
    }

    #[test]
    fn an_apply_cut_short_after_the_change_is_confirmed_and_can_be_reverted_at_the_next_start() {
        use volt_core::{BatchId, NewEntry, Operation, Timestamp};

        let system = FakeSystem::new();
        // What a killed helper leaves: the journal wrote the previous value (none) and the entry
        // is still pending, and the change itself had already been made.
        let batch = system
            .journal
            .new_batch(Some("machine-flag"))
            .expect("batch");
        let prior = Operation::SetRegistryValue {
            target: flag(),
            value: RegistryValue::Dword(1),
        }
        .detect(&system.ports())
        .expect("prior");
        let entry = system
            .journal
            .record_pending(NewEntry {
                batch: BatchId(batch.0),
                timestamp: Timestamp::from_unix_millis(1),
                source: Source::Manual,
                operation: Operation::SetRegistryValue {
                    target: flag(),
                    value: RegistryValue::Dword(1),
                },
                prior,
            })
            .expect("pending");
        system.registry.set(&flag(), RegistryValue::Dword(1));

        let desk = TweakDesk::new(DeskParts {
            catalogue: Arc::new(Catalogue::from_toml(CATALOGUE).expect("catalogue")),
            ports: system.ports(),
            journal: system.journal.clone(),
            activity: system.activity.clone(),
            clock: system.clock.clone(),
            confirmer: Arc::new(FakeConfirmer::accepting()),
            restore: Arc::new(volt_core::elevation::fakes::FakeRestorePoints::new()),
            store_id: "s".to_owned(),
        })
        .expect("desk");
        assert_eq!(desk.recovered().outcomes.len(), 1);
        assert_eq!(
            desk.recovered().outcomes[0].resolution,
            volt_core::Resolution::ConfirmedApplied
        );
        assert_eq!(
            system.journal.entry(entry).expect("entry").map(|e| e.state),
            Some(EntryState::Applied)
        );

        // And the tweak can now be reverted from the journal.
        let mut desk = desk;
        let reverted = desk
            .revert("machine-flag", Instant::now())
            .expect("reverted");
        assert!(matches!(reverted, ReplyBody::TweakRevert(r) if r.complete));
        assert_eq!(system.registry.get(&flag()), None);
    }

    #[test]
    fn an_apply_cut_short_before_the_change_leaves_nothing_to_revert() {
        use volt_core::{BatchId, NewEntry, Operation, Timestamp};

        let system = FakeSystem::new();
        let batch = system
            .journal
            .new_batch(Some("machine-flag"))
            .expect("batch");
        let prior = Operation::SetRegistryValue {
            target: flag(),
            value: RegistryValue::Dword(1),
        }
        .detect(&system.ports())
        .expect("prior");
        system
            .journal
            .record_pending(NewEntry {
                batch: BatchId(batch.0),
                timestamp: Timestamp::from_unix_millis(1),
                source: Source::Manual,
                operation: Operation::SetRegistryValue {
                    target: flag(),
                    value: RegistryValue::Dword(1),
                },
                prior,
            })
            .expect("pending");
        let desk = TweakDesk::new(DeskParts {
            catalogue: Arc::new(Catalogue::from_toml(CATALOGUE).expect("catalogue")),
            ports: system.ports(),
            journal: system.journal.clone(),
            activity: Arc::new(FakeActivityLog::new()),
            clock: system.clock.clone(),
            confirmer: Arc::new(FakeConfirmer::accepting()),
            restore: Arc::new(volt_core::elevation::fakes::FakeRestorePoints::new()),
            store_id: "s".to_owned(),
        })
        .expect("desk");
        assert_eq!(
            desk.recovered().outcomes[0].resolution,
            volt_core::Resolution::NeverApplied
        );
        assert!(
            system
                .journal
                .entries_in_state(EntryState::Pending)
                .expect("entries")
                .is_empty()
        );
    }

    #[test]
    fn the_real_store_keeps_an_interrupted_apply_across_a_restart_and_the_next_start_settles_it() {
        use volt_core::{BatchId, NewEntry, Operation, PriorState, Timestamp};
        use volt_store::Store;

        let dir = tempfile::tempdir().expect("temp");
        let path = dir.path().join("helper.db");
        let system = FakeSystem::new();
        let operation = Operation::SetRegistryValue {
            target: flag(),
            value: RegistryValue::Dword(1),
        };
        let prior: PriorState = operation.detect(&system.ports()).expect("prior");

        // First start: the journal gets the pending entry (write-ahead), then the helper is
        // "killed": the store is dropped with the entry still pending.
        let first_id = {
            let store = Arc::new(Store::open(&path).expect("open").store);
            let batch = store.new_batch(Some("machine-flag")).expect("batch");
            store
                .record_pending(NewEntry {
                    batch: BatchId(batch.0),
                    timestamp: Timestamp::from_unix_millis(1),
                    source: Source::Manual,
                    operation,
                    prior,
                })
                .expect("pending");
            store.store_id().expect("id")
        };
        // The change itself had been made before the kill.
        system.registry.set(&flag(), RegistryValue::Dword(1));

        // Next start: the same file, the same store identifier, and the desk settles the entry.
        let store = Arc::new(Store::open(&path).expect("reopen").store);
        assert_eq!(store.store_id().expect("id"), first_id);
        let mut desk = TweakDesk::new(DeskParts {
            catalogue: Arc::new(Catalogue::from_toml(CATALOGUE).expect("catalogue")),
            ports: system.ports(),
            journal: store.clone(),
            activity: store.clone(),
            clock: system.clock.clone(),
            confirmer: Arc::new(FakeConfirmer::accepting()),
            restore: Arc::new(volt_core::elevation::fakes::FakeRestorePoints::new()),
            store_id: first_id,
        })
        .expect("desk");
        assert_eq!(
            desk.recovered().outcomes[0].resolution,
            volt_core::Resolution::ConfirmedApplied
        );
        assert!(
            store
                .entries_in_state(EntryState::Pending)
                .expect("entries")
                .is_empty()
        );
        // The previous value came from the journal on disk.
        desk.revert("machine-flag", Instant::now()).expect("revert");
        assert_eq!(system.registry.get(&flag()), None);
    }
    #[test]
    fn a_caution_tweak_gets_a_restore_point_after_the_confirmation_and_before_any_change() {
        let mut rig = rig(FakeConfirmer::accepting());
        rig.desk.apply("machine-flag", rig.start).expect("applied");
        assert_eq!(
            rig.restore.descriptions(),
            vec!["Volt: Machine flag".to_owned()]
        );
        // A revert needs none, and a second apply within ten minutes does not make another.
        rig.desk
            .revert("machine-flag", rig.start)
            .expect("reverted");
        rig.desk
            .apply("machine-flag", rig.start + Duration::from_secs(60))
            .expect("applied again");
        assert_eq!(rig.restore.attempts(), 1);
    }

    #[test]
    fn a_declined_confirmation_makes_no_restore_point() {
        let mut rig = rig(FakeConfirmer::denying());
        assert_eq!(
            rig.desk.apply("machine-flag", rig.start),
            Err(ErrorCode::NotConfirmed)
        );
        assert_eq!(rig.restore.attempts(), 0);
    }

    #[test]
    fn without_a_restore_point_a_caution_tweak_is_refused_and_nothing_changes() {
        let mut rig = rig_with(FakeConfirmer::accepting(), FakeRestorePoints::failing());
        assert_eq!(
            rig.desk.apply("machine-flag", rig.start),
            Err(ErrorCode::RestorePointFailed)
        );
        assert_eq!(rig.system.registry.get(&flag()), None, "nothing changed");
        assert!(
            rig.system
                .journal
                .entries_in_state(EntryState::Pending)
                .expect("entries")
                .is_empty()
        );
        let records = rig.system.activity.records();
        assert_eq!(records.len(), 1, "{records:?}");
        assert_eq!(records[0].outcome, ActivityOutcome::Failed);
        assert!(
            records[0].summary.contains("restore point"),
            "{}",
            records[0].summary
        );
        assert_eq!(
            records[0]
                .phrase
                .as_ref()
                .map(|phrase| phrase.code.as_str()),
            Some("summary.adminRefused")
        );
        // The next try asks Windows again: a failure is not remembered as a point.
        assert_eq!(
            rig.desk
                .apply("machine-flag", rig.start + Duration::from_secs(1)),
            Err(ErrorCode::RestorePointFailed)
        );
        assert_eq!(rig.restore.attempts(), 2);
    }
}
