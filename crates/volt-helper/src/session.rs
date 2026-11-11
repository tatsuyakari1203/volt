//! One connection's worth of protocol: decode, order, quotas and the requests themselves.
//!
//! A [`Session`] takes the bytes of one request message and returns what to send back and whether
//! to hang up. It knows nothing about pipes or clocks (the time is passed in), so every rule here
//! is tested without either. The serve loop ([`crate::serve`]) wraps it in I/O and timers.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use volt_core::journal::Source;
use volt_core::{
    CatalogueHashes, ErrorCode, HelloReply, Memory, MemoryArea, MemoryCleaner, MemoryPurgeReply,
    PROTOCOL_VERSION, Reply, ReplyBody, Request, RequestSequencer, SystemClock, decode_request,
    encode_reply,
};

use crate::tweaks::TweakDesk;

/// How often the requests that change something may come.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuotaLimits {
    /// The least time between two memory purges.
    pub memory_purge_gap: Duration,
    /// Changing requests allowed in any window of [`Self::window`].
    pub changes_per_window: usize,
    /// The sliding window for [`Self::changes_per_window`].
    pub window: Duration,
}

impl Default for QuotaLimits {
    /// One memory purge per 5 seconds and 30 changing requests per minute.
    fn default() -> Self {
        Self {
            memory_purge_gap: Duration::from_secs(5),
            changes_per_window: 30,
            window: Duration::from_secs(60),
        }
    }
}

/// Remembers when the requests that change something were served, and says when one is too soon.
#[derive(Debug)]
pub struct Quotas {
    limits: QuotaLimits,
    last_purge: Option<Instant>,
    changes: VecDeque<Instant>,
}

impl Quotas {
    /// Quotas with nothing used yet.
    #[must_use]
    pub fn new(limits: QuotaLimits) -> Self {
        Self {
            limits,
            last_purge: None,
            changes: VecDeque::new(),
        }
    }

    /// Forgets the changes that have left the sliding window.
    fn expire(&mut self, now: Instant) {
        while self
            .changes
            .front()
            .is_some_and(|at| now.saturating_duration_since(*at) >= self.limits.window)
        {
            self.changes.pop_front();
        }
    }

    /// Whether a request that changes the system (an administrator tweak) may run at `now`: only
    /// the per-minute cap applies. When it may, the use is recorded; when it may not, nothing is.
    pub fn allow_change(&mut self, now: Instant) -> bool {
        self.expire(now);
        if self.changes.len() >= self.limits.changes_per_window {
            return false;
        }
        self.changes.push_back(now);
        true
    }

    /// Whether a memory purge may run at `now`. When it may, the use is recorded; when it may not,
    /// nothing is recorded, so a refused request never makes the next one wait longer.
    pub fn allow_memory_purge(&mut self, now: Instant) -> bool {
        self.expire(now);
        let too_soon = self
            .last_purge
            .is_some_and(|at| now.saturating_duration_since(at) < self.limits.memory_purge_gap);
        if too_soon || self.changes.len() >= self.limits.changes_per_window {
            return false;
        }
        self.last_purge = Some(now);
        self.changes.push_back(now);
        true
    }
}

/// What the serve loop does with one request message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handled {
    /// The reply message to send.
    pub reply: Vec<u8>,
    /// Hang up after sending it.
    pub close: bool,
    /// The request was valid and carried out. Only these keep the helper's idle clock from
    /// running: a refused request never extends the session.
    pub accepted: bool,
}

/// The state of one connection.
pub struct Session {
    cleaner: MemoryCleaner,
    hello: HelloReply,
    sequencer: RequestSequencer,
    greeted: bool,
    quotas: Quotas,
    tweaks: Option<TweakDesk>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("greeted", &self.greeted)
            .field("last_id", &self.sequencer.last())
            .finish_non_exhaustive()
    }
}

impl Session {
    /// A session whose memory purges go to `memory`, that reports `catalogues` in `hello`.
    ///
    /// The cleaner has no activity log: the helper's own journal and log belong to a later task.
    #[must_use]
    pub fn new(
        memory: Arc<dyn Memory>,
        catalogues: CatalogueHashes,
        helper_version: &str,
        quotas: QuotaLimits,
    ) -> Self {
        Self {
            cleaner: MemoryCleaner::new(memory, Arc::new(SystemClock)),
            hello: HelloReply {
                protocol: PROTOCOL_VERSION,
                helper_version: helper_version.to_owned(),
                catalogues,
            },
            sequencer: RequestSequencer::new(),
            greeted: false,
            quotas: Quotas::new(quotas),
            tweaks: None,
        }
    }

    /// The same session, serving the administrator-tweak requests through `desk`. Without a desk
    /// those requests are answered [`ErrorCode::Unavailable`].
    #[must_use]
    pub fn with_tweaks(mut self, desk: TweakDesk) -> Self {
        self.tweaks = Some(desk);
        self
    }

    fn reply(id: u64, body: ReplyBody, close: bool, accepted: bool) -> Handled {
        Handled {
            reply: encode_reply(&Reply { id, body }),
            close,
            accepted,
        }
    }

    /// A refusal. Whether the connection ends follows from the code.
    fn refuse(id: u64, code: ErrorCode) -> Handled {
        Self::reply(
            id,
            ReplyBody::Error { code },
            code.closes_connection(),
            false,
        )
    }

    /// The refusal for a message that was longer than the limit and so was never read.
    #[must_use]
    pub fn too_large() -> Handled {
        Self::refuse(0, ErrorCode::TooLarge)
    }

    /// Handles one request message received at `now`.
    pub fn handle(&mut self, message: &[u8], now: Instant) -> Handled {
        let envelope = match decode_request(message) {
            Ok(envelope) => envelope,
            Err(code) => return Self::refuse(0, code),
        };
        let id = envelope.id;
        if let Err(code) = self.sequencer.accept(id) {
            return Self::refuse(id, code);
        }
        match envelope.request {
            Request::Hello { protocol } => {
                if self.greeted {
                    return Self::refuse(id, ErrorCode::InvalidRequest);
                }
                if protocol != PROTOCOL_VERSION {
                    return Self::refuse(id, ErrorCode::ProtocolMismatch);
                }
                self.greeted = true;
                Self::reply(id, ReplyBody::Hello(self.hello.clone()), false, true)
            }
            Request::MemoryPurge { areas } => {
                if !self.greeted {
                    return Self::refuse(id, ErrorCode::HelloRequired);
                }
                if !valid_areas(&areas) {
                    return Self::refuse(id, ErrorCode::InvalidRequest);
                }
                if !self.quotas.allow_memory_purge(now) {
                    return Self::refuse(id, ErrorCode::RateLimited);
                }
                let report = self.cleaner.run(&areas, Source::Manual);
                Self::reply(
                    id,
                    ReplyBody::MemoryPurge(MemoryPurgeReply::from(&report)),
                    false,
                    true,
                )
            }
            Request::TweakApply { tweak } => self.change(id, now, &tweak, TweakDesk::apply),
            Request::TweakRevert { tweak } => self.change(id, now, &tweak, TweakDesk::revert),
            Request::History { after, limit } => {
                if !self.greeted {
                    return Self::refuse(id, ErrorCode::HelloRequired);
                }
                if limit == 0 {
                    return Self::refuse(id, ErrorCode::InvalidRequest);
                }
                let Some(desk) = &self.tweaks else {
                    return Self::refuse(id, ErrorCode::Unavailable);
                };
                match desk.history(after, limit) {
                    Ok(body) => Self::reply(id, body, false, true),
                    Err(code) => Self::refuse(id, code),
                }
            }
            Request::Bye {} => Self::reply(id, ReplyBody::Bye {}, true, true),
        }
    }

    /// An administrator-tweak request that changes the system: order, id syntax and the per-minute
    /// quota are checked here, then the desk confirms and acts. A request that the desk refuses
    /// (not confirmed, blocked, unknown, nothing to do) counts as refused: it never extends the
    /// session.
    fn change(
        &mut self,
        id: u64,
        now: Instant,
        tweak: &str,
        run: fn(&mut TweakDesk, &str, Instant) -> Result<ReplyBody, ErrorCode>,
    ) -> Handled {
        if !self.greeted {
            return Self::refuse(id, ErrorCode::HelloRequired);
        }
        if !valid_tweak_id(tweak) {
            return Self::refuse(id, ErrorCode::InvalidRequest);
        }
        if self.tweaks.is_none() {
            return Self::refuse(id, ErrorCode::Unavailable);
        }
        if !self.quotas.allow_change(now) {
            return Self::refuse(id, ErrorCode::RateLimited);
        }
        let Some(desk) = self.tweaks.as_mut() else {
            return Self::refuse(id, ErrorCode::Unavailable);
        };
        match run(desk, tweak, now) {
            Ok(body) => Self::reply(id, body, false, true),
            Err(code) => Self::refuse(id, code),
        }
    }
}

/// A tweak id as the catalogue spells it: lower-case letters, digits and hyphens, at most 64.
fn valid_tweak_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Areas of a purge: at least one, each at most once.
fn valid_areas(areas: &[MemoryArea]) -> bool {
    !areas.is_empty()
        && areas.len() <= MemoryArea::RUN_ORDER.len()
        && areas
            .iter()
            .enumerate()
            .all(|(index, area)| !areas[..index].contains(area))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use volt_core::fakes::FakeMemory;
    use volt_core::{AreaStatus, PortError, decode_reply};

    use super::*;

    fn catalogues() -> CatalogueHashes {
        CatalogueHashes {
            tweaks: "t".into(),
            cleaning: "c".into(),
        }
    }

    fn session_over(memory: Arc<FakeMemory>) -> Session {
        Session::new(memory, catalogues(), "0.0.0-test", QuotaLimits::default())
    }

    fn send(session: &mut Session, json: &str, now: Instant) -> (Reply, Handled) {
        let handled = session.handle(json.as_bytes(), now);
        (decode_reply(&handled.reply).expect("a reply"), handled)
    }

    fn code(reply: &Reply) -> ErrorCode {
        match &reply.body {
            ReplyBody::Error { code } => *code,
            other => panic!("expected an error, got {other:?}"),
        }
    }

    const HELLO: &str = r#"{"id":1,"request":{"type":"hello","protocol":1}}"#;

    fn greeted(memory: Arc<FakeMemory>) -> Session {
        let mut session = session_over(memory);
        let (reply, handled) = send(&mut session, HELLO, Instant::now());
        assert!(matches!(reply.body, ReplyBody::Hello(_)));
        assert!(handled.accepted && !handled.close);
        session
    }

    #[test]
    fn hello_reports_the_protocol_the_version_and_the_catalogue_hashes() {
        let mut session = session_over(Arc::new(FakeMemory::new()));
        let (reply, handled) = send(&mut session, HELLO, Instant::now());
        assert_eq!(reply.id, 1);
        let ReplyBody::Hello(hello) = reply.body else {
            panic!("expected hello");
        };
        assert_eq!(hello.protocol, PROTOCOL_VERSION);
        assert_eq!(hello.helper_version, "0.0.0-test");
        assert_eq!(hello.catalogues, catalogues());
        assert!(handled.accepted && !handled.close);
    }

    #[test]
    fn a_purge_runs_on_the_memory_port_and_reports_before_and_after() {
        let memory = Arc::new(FakeMemory::new());
        let mut session = greeted(Arc::clone(&memory));
        let (reply, handled) = send(
            &mut session,
            r#"{"id":2,"request":{"type":"memory_purge","areas":["StandbyList","WorkingSets"]}}"#,
            Instant::now(),
        );
        assert!(handled.accepted && !handled.close);
        let ReplyBody::MemoryPurge(report) = reply.body else {
            panic!("expected a purge report");
        };
        // The run order is the port's, not the request's.
        assert_eq!(
            memory.purged(),
            vec![MemoryArea::WorkingSets, MemoryArea::StandbyList]
        );
        assert_eq!(report.areas.len(), 2);
        assert!(report.any_purged());
        assert!(
            report.after.expect("after").free_bytes > report.before.expect("before").free_bytes
        );
        assert!(matches!(
            report.areas[0].status,
            AreaStatus::Purged {
                processes_trimmed: 2,
                processes_skipped: 1
            }
        ));
    }

    #[test]
    fn an_area_that_fails_is_reported_and_the_others_still_run() {
        let memory = Arc::new(FakeMemory::new());
        memory.fail_purge(
            MemoryArea::ModifiedList,
            PortError::AccessDenied("no privilege".into()),
        );
        let mut session = greeted(Arc::clone(&memory));
        let (reply, _) = send(
            &mut session,
            r#"{"id":2,"request":{"type":"memory_purge","areas":["ModifiedList","StandbyList"]}}"#,
            Instant::now(),
        );
        let ReplyBody::MemoryPurge(report) = reply.body else {
            panic!("expected a purge report");
        };
        assert!(matches!(report.areas[0].status, AreaStatus::Failed { .. }));
        assert!(matches!(report.areas[1].status, AreaStatus::Purged { .. }));
        assert_eq!(memory.purged(), vec![MemoryArea::StandbyList]);
    }

    #[test]
    fn nothing_but_hello_and_bye_works_before_hello() {
        let memory = Arc::new(FakeMemory::new());
        let mut session = session_over(Arc::clone(&memory));
        let (reply, handled) = send(
            &mut session,
            r#"{"id":1,"request":{"type":"memory_purge","areas":["StandbyList"]}}"#,
            Instant::now(),
        );
        assert_eq!(code(&reply), ErrorCode::HelloRequired);
        assert!(!handled.accepted && !handled.close);
        assert!(memory.purged().is_empty());
        // Hello still works, as request 2.
        let (reply, _) = send(
            &mut session,
            r#"{"id":2,"request":{"type":"hello","protocol":1}}"#,
            Instant::now(),
        );
        assert!(matches!(reply.body, ReplyBody::Hello(_)));
    }

    #[test]
    fn a_second_hello_and_a_wrong_protocol_are_refused() {
        let mut session = session_over(Arc::new(FakeMemory::new()));
        let (reply, handled) = send(
            &mut session,
            r#"{"id":1,"request":{"type":"hello","protocol":99}}"#,
            Instant::now(),
        );
        assert_eq!(code(&reply), ErrorCode::ProtocolMismatch);
        assert!(handled.close && !handled.accepted);

        let mut session = greeted(Arc::new(FakeMemory::new()));
        let (reply, handled) = send(
            &mut session,
            r#"{"id":2,"request":{"type":"hello","protocol":1}}"#,
            Instant::now(),
        );
        assert_eq!(code(&reply), ErrorCode::InvalidRequest);
        assert!(!handled.close && !handled.accepted);
    }

    #[test]
    fn bad_areas_are_refused_without_running_anything() {
        let memory = Arc::new(FakeMemory::new());
        let mut session = greeted(Arc::clone(&memory));
        for (id, areas) in [
            (2, "[]"),
            (3, r#"["StandbyList","StandbyList"]"#),
            (
                4,
                r#"["WorkingSets","FileCache","ModifiedList","StandbyListLowPriority","StandbyList","WorkingSets"]"#,
            ),
        ] {
            let json =
                format!(r#"{{"id":{id},"request":{{"type":"memory_purge","areas":{areas}}}}}"#);
            let (reply, handled) = send(&mut session, &json, Instant::now());
            assert_eq!(code(&reply), ErrorCode::InvalidRequest, "{areas}");
            assert!(!handled.close && !handled.accepted);
        }
        assert!(memory.purged().is_empty());
    }

    #[test]
    fn ids_out_of_order_close_the_connection_and_run_nothing() {
        let memory = Arc::new(FakeMemory::new());
        let mut session = greeted(Arc::clone(&memory));
        for json in [
            // A repeat, a gap and a step back.
            r#"{"id":1,"request":{"type":"memory_purge","areas":["StandbyList"]}}"#,
            r#"{"id":4,"request":{"type":"memory_purge","areas":["StandbyList"]}}"#,
            r#"{"id":0,"request":{"type":"bye"}}"#,
        ] {
            let (reply, handled) = send(&mut session, json, Instant::now());
            assert_eq!(code(&reply), ErrorCode::OutOfSequence, "{json}");
            assert!(handled.close && !handled.accepted);
        }
        assert!(memory.purged().is_empty());
    }

    #[test]
    fn framing_violations_close_the_connection() {
        let mut session = session_over(Arc::new(FakeMemory::new()));
        for (message, expected) in [
            (&b"not json"[..], ErrorCode::Malformed),
            (
                br#"{"id":1,"request":{"type":"bye","path":"C:\\x"}}"#,
                ErrorCode::Malformed,
            ),
            (
                br#"{"id":1,"request":{"type":"restore_point"}}"#,
                ErrorCode::UnknownRequest,
            ),
            (&[b'{'; 2048], ErrorCode::TooLarge),
        ] {
            let handled = session.handle(message, Instant::now());
            let reply = decode_reply(&handled.reply).expect("reply");
            assert_eq!(code(&reply), expected);
            assert!(handled.close && !handled.accepted);
        }
        let handled = Session::too_large();
        assert_eq!(
            code(&decode_reply(&handled.reply).expect("reply")),
            ErrorCode::TooLarge
        );
        assert!(handled.close);
    }

    #[test]
    fn bye_is_answered_and_ends_the_session() {
        let mut session = greeted(Arc::new(FakeMemory::new()));
        let (reply, handled) = send(
            &mut session,
            r#"{"id":2,"request":{"type":"bye"}}"#,
            Instant::now(),
        );
        assert_eq!(reply.body, ReplyBody::Bye {});
        assert!(handled.close && handled.accepted);
    }

    #[test]
    fn purges_are_rate_limited_and_a_refusal_costs_nothing() {
        let memory = Arc::new(FakeMemory::new());
        let mut session = greeted(Arc::clone(&memory));
        let start = Instant::now();
        let purge = |id: u64| {
            format!(r#"{{"id":{id},"request":{{"type":"memory_purge","areas":["ModifiedList"]}}}}"#)
        };
        let (reply, handled) = send(&mut session, &purge(2), start);
        assert!(matches!(reply.body, ReplyBody::MemoryPurge(_)) && handled.accepted);

        let (reply, handled) = send(&mut session, &purge(3), start + Duration::from_secs(2));
        assert_eq!(code(&reply), ErrorCode::RateLimited);
        assert!(!handled.accepted && !handled.close);

        // The refused request did not move the gap: 5 s after the LAST PURGE it works again.
        let (reply, handled) = send(&mut session, &purge(4), start + Duration::from_secs(5));
        assert!(matches!(reply.body, ReplyBody::MemoryPurge(_)) && handled.accepted);
        assert_eq!(memory.purged().len(), 2);
    }

    #[test]
    fn the_per_minute_cap_holds_even_with_the_gap_respected() {
        let limits = QuotaLimits {
            memory_purge_gap: Duration::from_secs(1),
            changes_per_window: 3,
            window: Duration::from_secs(60),
        };
        let mut quotas = Quotas::new(limits);
        let start = Instant::now();
        assert!(quotas.allow_memory_purge(start));
        assert!(!quotas.allow_memory_purge(start + Duration::from_millis(500)));
        assert!(quotas.allow_memory_purge(start + Duration::from_secs(1)));
        assert!(quotas.allow_memory_purge(start + Duration::from_secs(2)));
        // Three in the window: the fourth waits until the first leaves it.
        assert!(!quotas.allow_memory_purge(start + Duration::from_secs(30)));
        assert!(!quotas.allow_memory_purge(start + Duration::from_secs(59)));
        assert!(quotas.allow_memory_purge(start + Duration::from_secs(61)));
    }

    #[test]
    fn the_default_quotas_are_the_decisions() {
        let limits = QuotaLimits::default();
        assert_eq!(limits.memory_purge_gap, Duration::from_secs(5));
        assert_eq!(limits.changes_per_window, 30);
        assert_eq!(limits.window, Duration::from_secs(60));
    }

    // ---- administrator tweaks over the wire ------------------------------------------------

    mod tweak_requests {
        use volt_core::elevation::fakes::FakeConfirmer;
        use volt_core::fakes::FakeSystem;
        use volt_core::{
            ApplyOutcome, Catalogue, RegistryHive, RegistryValue, RegistryValueRef, Verdict,
        };

        use super::*;
        use crate::tweaks::{DeskParts, TweakDesk};

        const CATALOGUE: &str = r#"
[[tweak]]
id = "machine-flag"
title = "Machine flag"
description = "Sets a machine-wide flag."
category = "privacy"
risk = "safe"
evidence = "measured"
evidence_note = "Test."
requires_admin = true

[[tweak.steps]]
kind = "registry_set"
hive = "HKLM"
subkey = 'SOFTWARE\VoltTest\Session'
name = "Flag"
value = { dword = 1 }
"#;

        fn flag() -> RegistryValueRef {
            RegistryValueRef::new(
                RegistryHive::LocalMachine,
                r"SOFTWARE\VoltTest\Session",
                "Flag",
            )
        }

        struct Rig {
            system: FakeSystem,
            confirmer: Arc<FakeConfirmer>,
            session: Session,
        }

        fn rig(confirmer: FakeConfirmer, quotas: QuotaLimits) -> Rig {
            let system = FakeSystem::new();
            let confirmer = Arc::new(confirmer);
            let desk = TweakDesk::new(DeskParts {
                catalogue: Arc::new(Catalogue::from_toml(CATALOGUE).expect("catalogue")),
                ports: system.ports(),
                journal: system.journal.clone(),
                activity: system.activity.clone(),
                clock: system.clock.clone(),
                confirmer: confirmer.clone(),
                restore: Arc::new(volt_core::elevation::fakes::FakeRestorePoints::new()),
                store_id: "store-9".to_owned(),
            })
            .expect("desk");
            let mut session = Session::new(
                Arc::new(FakeMemory::new()),
                catalogues(),
                "0.0.0-test",
                quotas,
            )
            .with_tweaks(desk);
            let (reply, _) = send(&mut session, HELLO, Instant::now());
            assert!(matches!(reply.body, ReplyBody::Hello(_)));
            Rig {
                system,
                confirmer,
                session,
            }
        }

        fn apply(id: u64, tweak: &str) -> String {
            format!(r#"{{"id":{id},"request":{{"type":"tweak_apply","tweak":"{tweak}"}}}}"#)
        }

        fn revert(id: u64, tweak: &str) -> String {
            format!(r#"{{"id":{id},"request":{{"type":"tweak_revert","tweak":"{tweak}"}}}}"#)
        }

        #[test]
        fn a_confirmed_apply_and_revert_work_and_each_is_a_valid_request() {
            let mut rig = rig(FakeConfirmer::accepting(), QuotaLimits::default());
            let (reply, handled) =
                send(&mut rig.session, &apply(2, "machine-flag"), Instant::now());
            let ReplyBody::TweakApply(applied) = reply.body else {
                panic!("expected an apply reply");
            };
            assert_eq!(applied.outcome, ApplyOutcome::Applied);
            assert!(handled.accepted && !handled.close);
            assert_eq!(
                rig.system.registry.get(&flag()),
                Some(RegistryValue::Dword(1))
            );

            let (reply, handled) =
                send(&mut rig.session, &revert(3, "machine-flag"), Instant::now());
            let ReplyBody::TweakRevert(reverted) = reply.body else {
                panic!("expected a revert reply");
            };
            assert!(reverted.complete && handled.accepted);
            assert_eq!(rig.system.registry.get(&flag()), None);
        }

        #[test]
        fn an_unconfirmed_request_changes_nothing_is_logged_and_never_extends_the_session() {
            let mut rig = rig(FakeConfirmer::unavailable(), QuotaLimits::default());
            let (reply, handled) =
                send(&mut rig.session, &apply(2, "machine-flag"), Instant::now());
            assert_eq!(code(&reply), ErrorCode::NotConfirmed);
            assert!(!handled.accepted && !handled.close);
            assert_eq!(rig.system.registry.get(&flag()), None);
            assert_eq!(rig.system.activity.records().len(), 1);
            // History carries the refusal, by cursor.
            let (reply, _) = send(
                &mut rig.session,
                r#"{"id":3,"request":{"type":"history","after":0,"limit":5}}"#,
                Instant::now(),
            );
            let ReplyBody::History(history) = reply.body else {
                panic!("expected a history reply");
            };
            assert_eq!(history.store, "store-9");
            assert_eq!(history.records.len(), 1);
            assert!(history.records[0].summary.contains("machine-flag"));
        }

        #[test]
        fn a_denial_blocks_the_same_request_without_asking_again() {
            let mut rig = rig(
                FakeConfirmer::scripted(vec![Verdict::Denied, Verdict::Accepted]),
                QuotaLimits::default(),
            );
            let start = Instant::now();
            let (reply, _) = send(&mut rig.session, &apply(2, "machine-flag"), start);
            assert_eq!(code(&reply), ErrorCode::NotConfirmed);
            let (reply, handled) = send(
                &mut rig.session,
                &apply(3, "machine-flag"),
                start + Duration::from_secs(60),
            );
            assert_eq!(code(&reply), ErrorCode::Blocked);
            assert!(!handled.close && !handled.accepted);
            assert_eq!(rig.confirmer.asked().len(), 1);
            assert_eq!(rig.system.registry.get(&flag()), None);
        }

        #[test]
        fn requests_that_cannot_be_served_are_refused_before_anyone_is_asked() {
            let mut rig = rig(FakeConfirmer::accepting(), QuotaLimits::default());
            let long = "a".repeat(65);
            for (id, tweak) in [
                (2, ""),
                (3, "../x"),
                (4, "Machine-Flag"),
                (5, long.as_str()),
            ] {
                let (reply, handled) = send(&mut rig.session, &apply(id, tweak), Instant::now());
                assert_eq!(code(&reply), ErrorCode::InvalidRequest, "{tweak:?}");
                assert!(!handled.close && !handled.accepted);
            }
            let (reply, _) = send(&mut rig.session, &apply(6, "no-such-tweak"), Instant::now());
            assert_eq!(code(&reply), ErrorCode::UnknownTweak);
            let (reply, _) = send(&mut rig.session, &revert(7, "machine-flag"), Instant::now());
            assert_eq!(code(&reply), ErrorCode::NothingToRevert);
            let (reply, _) = send(
                &mut rig.session,
                r#"{"id":8,"request":{"type":"history","after":0,"limit":0}}"#,
                Instant::now(),
            );
            assert_eq!(code(&reply), ErrorCode::InvalidRequest);
            assert!(rig.confirmer.asked().is_empty());
            assert!(rig.system.activity.records().is_empty());
        }

        #[test]
        fn a_tweak_request_before_hello_or_without_a_desk_is_refused() {
            let system = FakeSystem::new();
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
            let mut session = Session::new(
                Arc::new(FakeMemory::new()),
                catalogues(),
                "0.0.0-test",
                QuotaLimits::default(),
            )
            .with_tweaks(desk);
            let (reply, _) = send(&mut session, &apply(1, "machine-flag"), Instant::now());
            assert_eq!(code(&reply), ErrorCode::HelloRequired);
            assert_eq!(system.registry.get(&flag()), None);

            // A helper whose desk could not be built serves no tweak and no history.
            let mut bare = greeted(Arc::new(FakeMemory::new()));
            for json in [
                apply(2, "machine-flag"),
                revert(3, "machine-flag"),
                r#"{"id":4,"request":{"type":"history","after":0,"limit":1}}"#.to_owned(),
            ] {
                let (reply, handled) = send(&mut bare, &json, Instant::now());
                assert_eq!(code(&reply), ErrorCode::Unavailable, "{json}");
                assert!(!handled.close && !handled.accepted);
            }
        }

        #[test]
        fn tweak_requests_share_the_per_minute_cap_with_purges() {
            let quotas = QuotaLimits {
                memory_purge_gap: Duration::from_secs(1),
                changes_per_window: 2,
                window: Duration::from_secs(60),
            };
            let mut rig = rig(FakeConfirmer::unavailable(), quotas);
            let start = Instant::now();
            let (reply, _) = send(&mut rig.session, &apply(2, "machine-flag"), start);
            assert_eq!(code(&reply), ErrorCode::NotConfirmed);
            let (reply, _) = send(&mut rig.session, &apply(3, "machine-flag"), start);
            assert_eq!(code(&reply), ErrorCode::NotConfirmed);
            let (reply, handled) = send(&mut rig.session, &apply(4, "machine-flag"), start);
            assert_eq!(code(&reply), ErrorCode::RateLimited);
            assert!(!handled.close && !handled.accepted);
            // After the window the next one is asked again.
            let (reply, _) = send(
                &mut rig.session,
                &apply(5, "machine-flag"),
                start + Duration::from_secs(61),
            );
            assert_eq!(code(&reply), ErrorCode::NotConfirmed);
        }
    }
}
