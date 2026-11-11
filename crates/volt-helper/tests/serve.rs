//! The helper's serve loop over a REAL named pipe, in this process, with no elevation. The test
//! process plays the UI the helper is bound to; a child process (the binary's `probe` mode, which
//! exists only outside release builds) plays a client that is not.
//!
//! Nothing here starts anything elevated or touches memory lists: purges go to a fake.

#![cfg(all(windows, not(helper_release)))]

use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use volt_core::fakes::FakeMemory;
use volt_core::{
    AreaStatus, CatalogueHashes, ErrorCode, MemoryArea, Reply, ReplyBody, Request, RequestEnvelope,
    decode_reply, encode_request,
};
use volt_helper::serve::{ExitReason, Helper, Limits, ServeReport};
use volt_helper::session::{QuotaLimits, Session};
use volt_win::helper::args::PipeName;
use volt_win::helper::identity::{ProcessIdentity, inspect_current};
use volt_win::helper::pipe::PipeClient;

const WAIT: Option<Duration> = Some(Duration::from_secs(5));
/// "Ends at once" means well before the 60 s limits these tests set. The bound only proves the
/// helper ended by its own rule and not by the limit, so it is far above what a busy machine
/// needs: a tight bound here measures the machine, not the helper.
const PROMPT: Duration = Duration::from_secs(30);

fn me() -> ProcessIdentity {
    inspect_current().expect("own process").identity
}

fn session(memory: &Arc<FakeMemory>) -> Session {
    Session::new(
        Arc::clone(memory) as Arc<dyn volt_core::Memory>,
        CatalogueHashes {
            tweaks: "tweaks-hash".into(),
            cleaning: "cleaning-hash".into(),
        },
        "test",
        QuotaLimits::default(),
    )
}

fn limits(idle_ms: u64, hard_ms: u64) -> Limits {
    Limits {
        connect_deadline: Duration::from_secs(60),
        idle: Duration::from_millis(idle_ms),
        hard_cap: Duration::from_millis(hard_ms),
        write_timeout: Duration::from_secs(5),
        close_grace: Duration::from_secs(1),
    }
}

/// A helper serving on its own thread.
struct Running {
    name: PipeName,
    memory: Arc<FakeMemory>,
    thread: JoinHandle<(ServeReport, Duration)>,
}

impl Running {
    fn start(bound: ProcessIdentity, limits: Limits) -> Self {
        let name = PipeName::random().expect("name");
        let memory = Arc::new(FakeMemory::new());
        // The pipe exists before the thread starts, so a client can connect at once.
        let helper = Helper::bind(&name, bound).expect("bind");
        let session = session(&memory);
        let thread = thread::spawn(move || {
            let started = Instant::now();
            let report = helper.serve(session, &limits);
            (report, started.elapsed())
        });
        Self {
            name,
            memory,
            thread,
        }
    }

    fn connect(&self) -> Client {
        Client {
            pipe: PipeClient::open_waiting(&self.name, Duration::from_secs(5)).expect("open"),
            next: 1,
        }
    }

    fn finish(self) -> (ServeReport, Duration) {
        self.thread.join().expect("serve thread")
    }
}

struct Client {
    pipe: PipeClient,
    next: u64,
}

impl Client {
    fn send(&mut self, request: Request) -> Reply {
        let id = self.next;
        self.next += 1;
        let bytes = encode_request(&RequestEnvelope { id, request }).expect("encodes");
        self.send_raw(&bytes)
    }

    fn send_raw(&self, bytes: &[u8]) -> Reply {
        self.pipe.write_message(bytes, WAIT).expect("write");
        let reply = self.pipe.read_message(4096, WAIT).expect("read");
        decode_reply(&reply).expect("a reply")
    }

    fn hello(&mut self) -> Reply {
        self.send(Request::Hello { protocol: 1 })
    }
}

fn error_code(reply: &Reply) -> ErrorCode {
    match &reply.body {
        ReplyBody::Error { code } => *code,
        other => panic!("expected an error, got {other:?}"),
    }
}

#[test]
fn a_whole_session_hello_purge_bye() {
    let helper = Running::start(me(), Limits::default());
    let mut client = helper.connect();

    let ReplyBody::Hello(hello) = client.hello().body else {
        panic!("expected hello");
    };
    assert_eq!(hello.catalogues.tweaks, "tweaks-hash");

    let purge = client.send(Request::MemoryPurge {
        areas: vec![MemoryArea::StandbyList, MemoryArea::WorkingSets],
    });
    assert_eq!(purge.id, 2);
    let ReplyBody::MemoryPurge(report) = purge.body else {
        panic!("expected a purge report");
    };
    assert!(report.any_purged());
    assert!(matches!(report.areas[0].status, AreaStatus::Purged { .. }));

    assert_eq!(client.send(Request::Bye {}).body, ReplyBody::Bye {});
    drop(client);

    let (report, _) = helper.finish();
    assert_eq!(report.exit, ExitReason::Bye);
    assert_eq!(report.served_requests, 3);
    assert_eq!(report.refused_clients, 0);
}

#[test]
fn purges_reach_the_memory_port_in_run_order() {
    let helper = Running::start(me(), Limits::default());
    let memory = Arc::clone(&helper.memory);
    let mut client = helper.connect();
    client.hello();
    client.send(Request::MemoryPurge {
        areas: vec![MemoryArea::StandbyList, MemoryArea::ModifiedList],
    });
    client.send(Request::Bye {});
    drop(client);
    helper.finish();
    assert_eq!(
        memory.purged(),
        vec![MemoryArea::ModifiedList, MemoryArea::StandbyList]
    );
}

#[test]
fn an_id_out_of_order_is_refused_and_the_connection_closed() {
    let helper = Running::start(me(), Limits::default());
    let mut client = helper.connect();
    client.hello();
    // Skips id 2.
    let bytes = encode_request(&RequestEnvelope {
        id: 3,
        request: Request::MemoryPurge {
            areas: vec![MemoryArea::StandbyList],
        },
    })
    .expect("encodes");
    let reply = client.send_raw(&bytes);
    assert_eq!(error_code(&reply), ErrorCode::OutOfSequence);
    let memory = Arc::clone(&helper.memory);
    let (report, _) = helper.finish();
    assert_eq!(report.exit, ExitReason::ProtocolViolation);
    assert!(memory.purged().is_empty());
}

#[test]
fn an_oversize_message_is_refused_and_the_connection_closed() {
    let helper = Running::start(me(), Limits::default());
    let client = helper.connect();
    let reply = client.send_raw(&vec![b' '; 2048]);
    assert_eq!(error_code(&reply), ErrorCode::TooLarge);
    let (report, _) = helper.finish();
    assert_eq!(report.exit, ExitReason::ProtocolViolation);
    assert_eq!(report.served_requests, 0);
}

#[test]
fn unknown_requests_and_unknown_fields_are_refused() {
    for (json, expected) in [
        (
            r#"{"id":1,"request":{"type":"run","cmd":"calc.exe"}}"#,
            ErrorCode::UnknownRequest,
        ),
        (
            r#"{"id":1,"request":{"type":"hello","protocol":1,"path":"C:\\x"}}"#,
            ErrorCode::Malformed,
        ),
    ] {
        let helper = Running::start(me(), Limits::default());
        let client = helper.connect();
        let reply = client.send_raw(json.as_bytes());
        assert_eq!(error_code(&reply), expected, "{json}");
        let (report, _) = helper.finish();
        assert_eq!(report.exit, ExitReason::ProtocolViolation);
    }
}

#[test]
fn the_helper_ends_when_the_bound_client_disconnects() {
    let helper = Running::start(me(), Limits::default());
    let mut client = helper.connect();
    client.hello();
    drop(client);
    let (report, elapsed) = helper.finish();
    assert_eq!(report.exit, ExitReason::ClientDisconnected);
    assert!(elapsed < PROMPT);
}

#[test]
fn the_helper_ends_after_the_idle_limit() {
    // Nobody ever connects.
    let helper = Running::start(me(), limits(300, 60_000));
    let (report, elapsed) = helper.finish();
    assert_eq!(report.exit, ExitReason::Idle);
    assert!(elapsed >= Duration::from_millis(250), "{elapsed:?}");
    assert!(elapsed < PROMPT, "{elapsed:?}");

    // A client connects, says hello and then goes quiet.
    let helper = Running::start(me(), limits(400, 60_000));
    let mut client = helper.connect();
    client.hello();
    let (report, elapsed) = helper.finish();
    assert_eq!(report.exit, ExitReason::Idle);
    assert_eq!(report.served_requests, 1);
    assert!(elapsed < PROMPT, "{elapsed:?}");
    drop(client);
}

#[test]
fn the_helper_ends_at_the_hard_cap_even_when_the_client_is_active() {
    let helper = Running::start(me(), limits(60_000, 600));
    let mut client = helper.connect();
    client.hello();
    let (report, elapsed) = helper.finish();
    assert_eq!(report.exit, ExitReason::HardCap);
    assert!(elapsed >= Duration::from_millis(500), "{elapsed:?}");
    assert!(elapsed < PROMPT, "{elapsed:?}");
    drop(client);
}

#[test]
fn refused_requests_never_extend_the_idle_clock() {
    let helper = Running::start(me(), limits(2_000, 60_000));
    let mut client = helper.connect();
    client.hello();
    // Refused (no areas) but not fatal: the connection stays. If these counted as activity the
    // helper would live until about 1800 + 2000 ms; it must end at about 2000 ms.
    for pause in [1_000, 800] {
        thread::sleep(Duration::from_millis(pause));
        let reply = client.send(Request::MemoryPurge { areas: Vec::new() });
        assert_eq!(error_code(&reply), ErrorCode::InvalidRequest);
    }
    let (report, elapsed) = helper.finish();
    assert_eq!(report.exit, ExitReason::Idle);
    assert_eq!(report.served_requests, 1);
    assert!(elapsed < Duration::from_millis(3_500), "{elapsed:?}");
    drop(client);
}

#[test]
fn a_client_that_is_not_the_bound_process_is_refused_while_the_bound_one_is_served() {
    let helper = Running::start(me(), Limits::default());

    // Another process, same user, same session, Medium integrity: not the bound UI.
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_volt-helper"))
        .args(["probe", "--pipe", helper.name.as_str()])
        .output()
        .expect("run the probe");
    assert_eq!(
        output.status.code(),
        Some(1),
        "the intruder must be refused"
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("refused"));

    // The helper kept listening: the bound process is served right after.
    let mut client = helper.connect();
    assert!(matches!(client.hello().body, ReplyBody::Hello(_)));
    client.send(Request::Bye {});
    drop(client);

    let (report, _) = helper.finish();
    assert_eq!(report.exit, ExitReason::Bye);
    assert_eq!(report.refused_clients, 1);
    assert_eq!(report.served_requests, 2);
}

#[test]
fn work_that_is_already_waiting_does_not_get_past_an_expired_clock() {
    // A pipe call that finds its work waiting completes at once whatever its timeout, so the
    // clocks must also be checked before each call. Here a client has connected and sent hello
    // before the helper starts listening; both clocks are already over when it does.
    for (idle_ms, hard_ms) in [(0, 60_000), (60_000, 0)] {
        let name = PipeName::random().expect("name");
        let helper = Helper::bind(&name, me()).expect("bind");
        let memory = Arc::new(FakeMemory::new());
        let client = PipeClient::open(&name).expect("open");
        let hello = encode_request(&RequestEnvelope {
            id: 1,
            request: Request::Hello { protocol: 1 },
        })
        .expect("encodes");
        client.write_message(&hello, WAIT).expect("write");

        let report = helper.serve(session(&memory), &limits(idle_ms, hard_ms));
        assert_eq!(
            report.exit,
            if idle_ms == 0 {
                ExitReason::Idle
            } else {
                ExitReason::HardCap
            }
        );
        assert_eq!(report.served_requests, 0, "idle {idle_ms}, hard {hard_ms}");
        drop(client);
    }
}

#[test]
fn a_client_with_the_wrong_creation_time_or_image_is_refused() {
    let own = me();
    for bound in [
        // The right process id, but as if it were another process that reused it.
        ProcessIdentity {
            created_at_100ns: own.created_at_100ns + 1,
            ..own.clone()
        },
        // The right id and time, but another image than the one the helper expects.
        ProcessIdentity {
            image: format!("{}.other", own.image),
            ..own.clone()
        },
        // Another process id altogether.
        ProcessIdentity {
            pid: own.pid.wrapping_add(4),
            ..own.clone()
        },
    ] {
        let helper = Running::start(bound.clone(), limits(500, 60_000));
        let client = helper.connect();
        // The connection is taken and dropped; nothing is ever answered.
        let _ = client.pipe.write_message(b"{}", WAIT);
        assert!(client.pipe.read_message(4096, WAIT).is_err(), "{bound:?}");
        let (report, _) = helper.finish();
        assert_eq!(report.exit, ExitReason::Idle, "{bound:?}");
        assert_eq!(report.refused_clients, 1, "{bound:?}");
        assert_eq!(report.served_requests, 0);
    }
}

#[test]
fn the_helper_ends_when_the_bound_ui_never_connects() {
    // A stranger does not count as the UI: it is refused, and the deadline still runs.
    let helper = Running::start(
        me(),
        Limits {
            // The stranger's probe is a new process; on a busy machine it can take a while to
            // start, and the deadline must not pass before it has been refused.
            connect_deadline: Duration::from_secs(4),
            ..limits(60_000, 60_000)
        },
    );
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_volt-helper"))
        .args(["probe", "--pipe", helper.name.as_str()])
        .output()
        .expect("run the probe");
    assert_eq!(output.status.code(), Some(1));
    let (report, elapsed) = helper.finish();
    assert_eq!(report.exit, ExitReason::NoConnection);
    assert_eq!(report.refused_clients, 1);
    assert_eq!(report.served_requests, 0);
    assert!(elapsed >= Duration::from_millis(3_900), "{elapsed:?}");
    assert!(elapsed < PROMPT, "{elapsed:?}");
}

#[test]
fn the_connection_deadline_does_not_cut_a_session_that_has_begun() {
    let helper = Running::start(
        me(),
        Limits {
            connect_deadline: Duration::from_millis(300),
            ..limits(1500, 60_000)
        },
    );
    let mut client = helper.connect();
    client.hello();
    thread::sleep(Duration::from_millis(600));
    let purge = client.send(Request::MemoryPurge {
        areas: vec![MemoryArea::StandbyList],
    });
    assert!(matches!(purge.body, ReplyBody::MemoryPurge(_)));
    client.send(Request::Bye {});
    drop(client);
    let (report, _) = helper.finish();
    assert_eq!(report.exit, ExitReason::Bye);
}

#[test]
fn the_helper_ends_at_once_when_the_bound_ui_process_exits() {
    // A throwaway child plays the UI: the probe waits on its pipe for a while.
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_volt-helper"))
        .args(["sleep-selftest"])
        .spawn()
        .expect("start the child");
    let facts = volt_win::helper::identity::inspect_process(child.id()).expect("child facts");
    let helper = Running::start(facts.identity, limits(60_000, 60_000));
    // Not yet connected, the child alive: the helper waits.
    thread::sleep(Duration::from_millis(500));
    assert!(!helper.thread.is_finished());
    child.kill().expect("end the child");
    child.wait().expect("reaped");
    let (report, elapsed) = helper.finish();
    assert_eq!(report.exit, ExitReason::UiExited);
    assert!(elapsed < PROMPT, "{elapsed:?}");
}
