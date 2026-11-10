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

    /// Whether a memory purge may run at `now`. When it may, the use is recorded; when it may not,
    /// nothing is recorded, so a refused request never makes the next one wait longer.
    pub fn allow_memory_purge(&mut self, now: Instant) -> bool {
        while self
            .changes
            .front()
            .is_some_and(|at| now.saturating_duration_since(*at) >= self.limits.window)
        {
            self.changes.pop_front();
        }
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
        }
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
            Request::Bye {} => Self::reply(id, ReplyBody::Bye {}, true, true),
        }
    }
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
}
