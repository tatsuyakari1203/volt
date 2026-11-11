//! The real [`Elevator`]: one helper connection per UI session.
//!
//! [`WinElevator`] starts the helper only when a user request needs it, through the one
//! administrator prompt, and then keeps the connection:
//!
//! 1. **Availability.** The checks of [`launch`](super::launch) must pass; otherwise no prompt is
//!    ever started and the typed reason comes back.
//! 2. **Launch.** The helper image is the sibling of this executable. Its arguments are a fresh
//!    random pipe name, this process's id and creation time ([`HelperArgs`]). The prompt cancelled
//!    is [`ElevationError::Declined`].
//! 3. **Authenticate the server BEFORE the first request.** The pipe is opened with a bounded
//!    wait; the process serving it must be the process the launch call returned, and the pipe's
//!    owner must be Administrators or SYSTEM (which a medium-integrity process cannot create).
//!    When either cannot be shown, nothing is sent: [`ElevationError::Untrusted`].
//! 4. **hello.** The protocol version and both catalogue hashes must equal this build's, otherwise
//!    [`ElevationError::Mismatch`] and the connection is dropped.
//!
//! Later requests reuse the connection while the helper is alive (its process has not exited and
//! its end of the pipe is open). When it is not, the NEXT user request starts a new helper (a new
//! prompt). A request that breaks in the middle is [`ElevationError::Disconnected`] and is never
//! retried by itself, so one click can never show two prompts. [`Elevator::bye`] says bye and
//! closes; the app calls it on exit.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError, TryLockError};
use std::time::Duration;

use volt_core::{
    CatalogueHashes, ElevationError, ElevationStatus, Elevator, ErrorCode, HelloReply,
    HistoryReply, MAX_REPLY_BYTES, MemoryArea, MemoryPurgeReply, PROTOCOL_VERSION, ReplyBody,
    Request, RequestEnvelope, TweakApplyReply, TweakRevertReply, UnavailableReason, decode_reply,
    encode_request,
};

use super::args::{HelperArgs, PipeName};
use super::identity::{ProcessHandle, inspect_current};
use super::launch::{HELPER_IMAGE_NAME, LaunchError, Launcher, ShellLauncher, status_for};
use super::pipe::{PipeClient, is_trusted_owner};

/// How long to wait for the helper to create its pipe after the prompt was answered.
const OPEN_WAIT: Duration = Duration::from_secs(10);
/// The longest a request or the hello may take, and the longest a request may be written for.
const SHORT_TIMEOUT: Duration = Duration::from_secs(30);
/// The longest a purge may take (the decision's per-request limit).
const PURGE_TIMEOUT: Duration = Duration::from_secs(120);
/// The longest a tweak request may take: the helper waits for a person to answer its confirmation
/// first (the decision's per-request limit).
const CHANGE_TIMEOUT: Duration = Duration::from_secs(120);
/// How long saying bye may take.
const BYE_TIMEOUT: Duration = Duration::from_secs(2);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// An authenticated connection to a helper that passed `hello`.
#[derive(Debug)]
struct Connection {
    client: PipeClient,
    process: ProcessHandle,
    hello: HelloReply,
    /// The id of the next request.
    next_id: u64,
    /// What the CI job measures about this connection.
    #[cfg(feature = "elevated-ci")]
    recorded: Recorded,
}

/// What a connection records for the CI job: the pipe and the two times.
#[cfg(feature = "elevated-ci")]
#[derive(Debug)]
struct Recorded {
    pipe: PipeName,
    /// How long the launch call took (it returns when the process has been created).
    launch_call: Duration,
    /// From before the launch call to the `hello` reply.
    launch_to_hello: Duration,
}

impl Connection {
    /// Whether the helper is still there: its process has not exited and its end of the pipe is
    /// open.
    fn is_alive(&self) -> bool {
        !self.process.has_exited() && self.client.is_open()
    }

    /// Sends one request and returns the answer's body. The id is spent whatever happens; a failure
    /// leaves the connection to be dropped by the caller.
    fn exchange(
        &mut self,
        request: Request,
        timeout: Duration,
    ) -> Result<ReplyBody, ElevationError> {
        let id = self.next_id;
        self.next_id += 1;
        exchange(&self.client, id, request, timeout)
    }
}

/// One request and its answer on `client`. An error reply is [`ElevationError::Refused`]; a broken
/// pipe, a timeout, a reply that cannot be read or one for another id is
/// [`ElevationError::Disconnected`].
fn exchange(
    client: &PipeClient,
    id: u64,
    request: Request,
    timeout: Duration,
) -> Result<ReplyBody, ElevationError> {
    let bytes =
        encode_request(&RequestEnvelope { id, request }).map_err(ElevationError::Refused)?;
    client
        .write_message(&bytes, Some(timeout))
        .map_err(|_| ElevationError::Disconnected)?;
    let message = client
        .read_message(MAX_REPLY_BYTES, Some(timeout))
        .map_err(|_| ElevationError::Disconnected)?;
    let reply = decode_reply(&message).map_err(|_| ElevationError::Disconnected)?;
    if reply.id != id {
        return Err(ElevationError::Disconnected);
    }
    match reply.body {
        ReplyBody::Error { code } => Err(ElevationError::Refused(code)),
        body => Ok(body),
    }
}

/// What a test may change. Compiled only for tests: a release build has no way to relax the checks.
#[cfg(test)]
#[derive(Debug, Default)]
struct TestHooks {
    /// Replaces the availability checks (a test binary is never in Program Files).
    status: Option<ElevationStatus>,
    /// A pipe owner that is trusted in addition to Administrators and SYSTEM (an unelevated test
    /// server is owned by the user).
    trusted_owner: Option<String>,
}

/// The real [`Elevator`], over the helper pipe. See the module documentation.
pub struct WinElevator {
    expected: CatalogueHashes,
    launcher: Box<dyn Launcher>,
    session: Mutex<Option<Connection>>,
    #[cfg(test)]
    test: TestHooks,
}

impl std::fmt::Debug for WinElevator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WinElevator").finish_non_exhaustive()
    }
}

impl WinElevator {
    /// An elevator that starts the real helper through the administrator prompt, and refuses a
    /// helper whose catalogue hashes differ from `expected` (the hashes of the catalogues this app
    /// embeds). Nothing is started until a request needs the helper.
    #[must_use]
    pub fn new(expected: CatalogueHashes) -> Self {
        Self::build(expected, Box::new(ShellLauncher))
    }

    fn build(expected: CatalogueHashes, launcher: Box<dyn Launcher>) -> Self {
        Self {
            expected,
            launcher,
            session: Mutex::new(None),
            #[cfg(test)]
            test: TestHooks::default(),
        }
    }

    #[cfg_attr(
        not(test),
        allow(
            clippy::unused_self,
            reason = "only a test build reads the hooks on self"
        )
    )]
    fn current_status(&self) -> ElevationStatus {
        #[cfg(test)]
        if let Some(status) = &self.test.status {
            return status.clone();
        }
        match inspect_current() {
            Ok(me) => status_for(Path::new(&me.identity.image)),
            Err(detail) => ElevationStatus::Unavailable {
                reason: UnavailableReason::Unknown { detail },
            },
        }
    }

    #[cfg_attr(
        not(test),
        allow(
            clippy::unused_self,
            reason = "only a test build reads the hooks on self"
        )
    )]
    fn owner_is_trusted(&self, owner: &str) -> bool {
        #[cfg(test)]
        if self.test.trusted_owner.as_deref() == Some(owner) {
            return true;
        }
        is_trusted_owner(owner)
    }

    /// The server must be the process that was started, and the pipe must belong to
    /// Administrators or SYSTEM. Cannot verify means nothing is sent.
    fn authenticate(
        &self,
        client: &PipeClient,
        process: &ProcessHandle,
    ) -> Result<(), ElevationError> {
        if !client.server_is(process) {
            return Err(ElevationError::Untrusted(
                "the pipe is served by another process than the helper that was started".to_owned(),
            ));
        }
        match client.owner_sid() {
            Ok(owner) if self.owner_is_trusted(&owner) => Ok(()),
            Ok(_) => Err(ElevationError::Untrusted(
                "the pipe is not owned by Administrators or SYSTEM".to_owned(),
            )),
            Err(reason) => Err(ElevationError::Untrusted(format!(
                "the pipe owner could not be read ({reason})"
            ))),
        }
    }

    /// Starts a helper (the prompt), authenticates it and agrees on the version.
    fn connect(&self) -> Result<Connection, ElevationError> {
        if let ElevationStatus::Unavailable { reason } = self.current_status() {
            return Err(ElevationError::Unavailable(reason.to_string()));
        }
        let unavailable = |what: &str| ElevationError::Unavailable(what.to_owned());
        let me = inspect_current().map_err(|_| unavailable("this app could not be identified"))?;
        let helper: PathBuf = Path::new(&me.identity.image).with_file_name(HELPER_IMAGE_NAME);
        let pipe = PipeName::random().map_err(|_| unavailable("no random pipe name"))?;
        let args = HelperArgs {
            pipe: pipe.clone(),
            ui_pid: me.identity.pid,
            ui_created_at_100ns: me.identity.created_at_100ns,
        }
        .render();

        #[cfg(feature = "elevated-ci")]
        let started = std::time::Instant::now();
        let process = match self.launcher.launch(&helper, &args) {
            Ok(process) => process,
            Err(LaunchError::Declined) => return Err(ElevationError::Declined),
            Err(LaunchError::Failed(reason)) => {
                return Err(ElevationError::Unavailable(format!(
                    "the helper could not be started ({reason})"
                )));
            }
        };
        #[cfg(feature = "elevated-ci")]
        let launch_call = started.elapsed();
        let client = PipeClient::open_waiting(&pipe, OPEN_WAIT).map_err(|_| {
            unavailable("the helper did not start serving (it may have refused to run here)")
        })?;
        self.authenticate(&client, &process)?;

        let hello = match exchange(
            &client,
            1,
            Request::Hello {
                protocol: PROTOCOL_VERSION,
            },
            SHORT_TIMEOUT,
        ) {
            Ok(ReplyBody::Hello(hello)) => hello,
            Err(ElevationError::Refused(ErrorCode::ProtocolMismatch)) => {
                return Err(ElevationError::Mismatch(
                    "the helper speaks another protocol version".to_owned(),
                ));
            }
            Ok(_) => return Err(ElevationError::Disconnected),
            Err(error) => return Err(error),
        };
        if hello.protocol != PROTOCOL_VERSION {
            return Err(ElevationError::Mismatch(
                "the helper speaks another protocol version".to_owned(),
            ));
        }
        if hello.catalogues != self.expected {
            return Err(ElevationError::Mismatch(
                "the helper holds other catalogues than this version of Volt".to_owned(),
            ));
        }
        Ok(Connection {
            client,
            process,
            hello,
            next_id: 2,
            #[cfg(feature = "elevated-ci")]
            recorded: Recorded {
                pipe,
                launch_call,
                launch_to_hello: started.elapsed(),
            },
        })
    }

    /// Runs `work` on the live connection, starting one (a prompt) when there is none or the helper
    /// has exited since. A connection that failed is dropped, so the next call starts over; this
    /// call is never repeated.
    fn with_session<T>(
        &self,
        work: impl FnOnce(&mut Connection) -> Result<T, ElevationError>,
    ) -> Result<T, ElevationError> {
        let mut session = lock(&self.session);
        if session
            .as_ref()
            .is_some_and(|connection| !connection.is_alive())
        {
            *session = None;
        }
        if session.is_none() {
            *session = Some(self.connect()?);
        }
        let Some(connection) = session.as_mut() else {
            return Err(ElevationError::Disconnected);
        };
        let result = work(connection);
        let broken = match &result {
            Err(ElevationError::Disconnected) => true,
            Err(ElevationError::Refused(code)) => code.closes_connection(),
            _ => false,
        };
        if broken {
            *session = None;
        }
        result
    }

    /// Says bye to the helper and closes the connection, when there is one and nobody is using it
    /// (a request in flight is left: closing the process ends the helper's session anyway).
    fn end_session(&self) {
        let taken = match self.session.try_lock() {
            Ok(mut session) => session.take(),
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner().take(),
            Err(TryLockError::WouldBlock) => None,
        };
        if let Some(mut connection) = taken {
            let _ = connection.exchange(Request::Bye {}, BYE_TIMEOUT);
        }
    }
}

impl Elevator for WinElevator {
    fn status(&self) -> ElevationStatus {
        self.current_status()
    }

    fn hello(&self) -> Result<HelloReply, ElevationError> {
        self.with_session(|connection| Ok(connection.hello.clone()))
    }

    fn memory_purge(&self, areas: &[MemoryArea]) -> Result<MemoryPurgeReply, ElevationError> {
        // Each area at most once, in the order given. Nothing to do means nothing to prompt for.
        let mut unique: Vec<MemoryArea> = Vec::new();
        for area in areas {
            if !unique.contains(area) {
                unique.push(*area);
            }
        }
        if unique.is_empty() {
            return Err(ElevationError::Refused(ErrorCode::InvalidRequest));
        }
        self.with_session(|connection| {
            match connection.exchange(Request::MemoryPurge { areas: unique }, PURGE_TIMEOUT)? {
                ReplyBody::MemoryPurge(reply) => Ok(reply),
                // An answer of another kind to a purge: the helper is not speaking the protocol.
                _ => Err(ElevationError::Refused(ErrorCode::Internal)),
            }
        })
    }

    fn tweak_apply(&self, tweak: &str) -> Result<TweakApplyReply, ElevationError> {
        self.with_session(|connection| {
            let request = Request::TweakApply {
                tweak: tweak.to_owned(),
            };
            match connection.exchange(request, CHANGE_TIMEOUT)? {
                ReplyBody::TweakApply(reply) => Ok(reply),
                _ => Err(ElevationError::Refused(ErrorCode::Internal)),
            }
        })
    }

    fn tweak_revert(&self, tweak: &str) -> Result<TweakRevertReply, ElevationError> {
        self.with_session(|connection| {
            let request = Request::TweakRevert {
                tweak: tweak.to_owned(),
            };
            match connection.exchange(request, CHANGE_TIMEOUT)? {
                ReplyBody::TweakRevert(reply) => Ok(reply),
                _ => Err(ElevationError::Refused(ErrorCode::Internal)),
            }
        })
    }

    fn history(&self, after: u64, limit: u32) -> Result<HistoryReply, ElevationError> {
        self.with_session(|connection| {
            match connection.exchange(Request::History { after, limit }, SHORT_TIMEOUT)? {
                ReplyBody::History(reply) => Ok(reply),
                _ => Err(ElevationError::Refused(ErrorCode::Internal)),
            }
        })
    }

    fn bye(&self) {
        self.end_session();
    }
}

/// The facts of a live connection that the CI job records.
#[cfg(feature = "elevated-ci")]
#[derive(Debug)]
pub struct ConnectionEvidence {
    /// The pipe the helper serves.
    pub pipe: PipeName,
    /// The helper's process id, from the launch handle.
    pub helper_pid: Option<u32>,
    /// The pipe's owner as the client reads it.
    pub owner_sid: Result<String, String>,
    /// Whether the owner is one the client trusts.
    pub owner_trusted: bool,
    /// The process id `GetNamedPipeServerProcessId` reports.
    pub server_pid: Result<u32, String>,
    /// Whether that process is the one the launch handle refers to.
    pub server_is_launched: bool,
    /// What the launch handle allows.
    pub rights: super::ci::HandleRights,
    /// The helper's memory now.
    pub working_set: Result<super::ci::WorkingSet, String>,
    /// How long the launch call took.
    pub launch_call: Duration,
    /// From before the launch call to the `hello` reply.
    pub launch_to_hello: Duration,
}

#[cfg(feature = "elevated-ci")]
impl WinElevator {
    /// What is true of the live connection now, or `None` when there is none. Never starts a
    /// helper.
    #[must_use]
    pub fn ci_evidence(&self) -> Option<ConnectionEvidence> {
        let session = lock(&self.session);
        let connection = session.as_ref()?;
        let owner_sid = connection.client.owner_sid();
        Some(ConnectionEvidence {
            pipe: connection.recorded.pipe.clone(),
            helper_pid: connection.process.pid().ok(),
            owner_trusted: owner_sid
                .as_deref()
                .is_ok_and(|owner| self.owner_is_trusted(owner)),
            owner_sid,
            server_pid: connection.client.server_pid(),
            server_is_launched: connection.client.server_is(&connection.process),
            rights: super::ci::handle_rights(&connection.process),
            working_set: super::ci::working_set(&connection.process),
            launch_call: connection.recorded.launch_call,
            launch_to_hello: connection.recorded.launch_to_hello,
        })
    }
}

impl Drop for WinElevator {
    fn drop(&mut self) {
        self.end_session();
    }
}

#[cfg(test)]
mod tests {
    use std::process::{Child, Command, Stdio};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::thread::{self, JoinHandle};

    use volt_core::fakes::FakeMemory;
    use volt_core::{
        AreaStatus, Memory, MemoryCleaner, Reply, Source, SystemClock, decode_request, encode_reply,
    };

    use super::super::args::parse_args;
    use super::super::identity::{TokenKind, current_token_kind};
    use super::super::pipe::PipeServer;
    use super::*;

    const SECOND: Option<Duration> = Some(Duration::from_secs(5));

    fn own_sid() -> String {
        inspect_current().expect("own token").token.user_sid
    }

    fn hashes(tag: &str) -> CatalogueHashes {
        CatalogueHashes {
            tweaks: format!("tweaks-{tag}"),
            cleaning: format!("cleaning-{tag}"),
        }
    }

    /// What one fake helper does.
    #[derive(Clone)]
    struct Plan {
        hashes: CatalogueHashes,
        protocol: u32,
        /// Serve this many requests (replying to each), then go away.
        exit_after: Option<usize>,
        /// Go away on receiving this request, without answering it.
        die_on: Option<usize>,
        /// The process handle the launch call hands back is not the one serving the pipe.
        foreign_process: bool,
    }

    impl Plan {
        fn genuine() -> Self {
            Self {
                hashes: hashes("a"),
                protocol: PROTOCOL_VERSION,
                exit_after: None,
                die_on: None,
                foreign_process: false,
            }
        }
    }

    /// How the replaced launch call behaves.
    #[derive(Clone)]
    enum Mode {
        Decline,
        Fail,
        Serve(Plan),
    }

    /// The replaced launch call: instead of the prompt and the helper it creates the real helper
    /// pipe in this process and serves it on a thread, with the real pipe code of part 1.
    #[derive(Clone)]
    struct TestLauncher {
        mode: Arc<Mutex<Mode>>,
        launches: Arc<AtomicU32>,
        memory: Arc<FakeMemory>,
        /// Every request any fake helper received, with its id.
        seen: Arc<Mutex<Vec<(u64, Request)>>>,
        servers: Arc<Mutex<Vec<JoinHandle<()>>>>,
        children: Arc<Mutex<Vec<Child>>>,
    }

    impl TestLauncher {
        fn new(mode: Mode) -> Self {
            Self {
                mode: Arc::new(Mutex::new(mode)),
                launches: Arc::new(AtomicU32::new(0)),
                memory: Arc::new(FakeMemory::new()),
                seen: Arc::new(Mutex::new(Vec::new())),
                servers: Arc::new(Mutex::new(Vec::new())),
                children: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn launches(&self) -> u32 {
            self.launches.load(Ordering::SeqCst)
        }

        fn seen(&self) -> Vec<(u64, Request)> {
            lock(&self.seen).clone()
        }

        /// Waits for every fake helper thread to finish.
        fn wait_for_helpers(&self) {
            let threads: Vec<_> = lock(&self.servers).drain(..).collect();
            for thread in threads {
                thread.join().expect("fake helper thread");
            }
        }
    }

    impl Drop for TestLauncher {
        fn drop(&mut self) {
            // Only the last clone cleans up.
            if Arc::strong_count(&self.children) == 1 {
                for child in lock(&self.children).iter_mut() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
        }
    }

    fn serve(
        server: &PipeServer,
        plan: &Plan,
        memory: &Arc<FakeMemory>,
        seen: &Arc<Mutex<Vec<(u64, Request)>>>,
    ) {
        if server.accept(SECOND).is_err() {
            return;
        }
        let mut handled = 0_usize;
        loop {
            let Ok(message) = server.read_message(volt_core::MAX_REQUEST_BYTES, SECOND) else {
                return;
            };
            let Ok(envelope) = decode_request(&message) else {
                return;
            };
            handled += 1;
            lock(seen).push((envelope.id, envelope.request.clone()));
            if plan.die_on == Some(handled) {
                return;
            }
            let body = match &envelope.request {
                Request::Hello { .. } => ReplyBody::Hello(HelloReply {
                    protocol: plan.protocol,
                    helper_version: "test".to_owned(),
                    catalogues: plan.hashes.clone(),
                }),
                Request::MemoryPurge { areas } => {
                    let cleaner = MemoryCleaner::new(
                        Arc::clone(memory) as Arc<dyn Memory>,
                        Arc::new(SystemClock),
                    );
                    ReplyBody::MemoryPurge(MemoryPurgeReply::from(
                        &cleaner.run(areas, Source::Manual),
                    ))
                }
                Request::TweakApply { tweak } => ReplyBody::TweakApply(TweakApplyReply {
                    tweak: tweak.clone(),
                    outcome: volt_core::ApplyOutcome::Applied,
                }),
                Request::TweakRevert { tweak } => ReplyBody::TweakRevert(TweakRevertReply {
                    tweak: tweak.clone(),
                    batches: 1,
                    complete: true,
                }),
                Request::History { .. } => ReplyBody::History(HistoryReply {
                    store: "test-store".to_owned(),
                    records: Vec::new(),
                    more: false,
                }),
                Request::Bye {} => ReplyBody::Bye {},
            };
            let reply = encode_reply(&Reply {
                id: envelope.id,
                body,
            });
            if server.write_message(&reply, SECOND).is_err() {
                return;
            }
            if matches!(envelope.request, Request::Bye {}) {
                server.finish(Duration::from_secs(2));
                return;
            }
            if plan.exit_after == Some(handled) {
                // Let the client read the last reply, then go away.
                server.finish(Duration::from_millis(200));
                return;
            }
        }
    }

    impl Launcher for TestLauncher {
        fn launch(&self, image: &Path, args: &[String]) -> Result<ProcessHandle, LaunchError> {
            self.launches.fetch_add(1, Ordering::SeqCst);
            let mode = lock(&self.mode).clone();
            let plan = match mode {
                Mode::Decline => return Err(LaunchError::Declined),
                Mode::Fail => return Err(LaunchError::Failed("blocked by policy".to_owned())),
                Mode::Serve(plan) => plan,
            };
            // The arguments are exactly what the helper's strict parser accepts, and the image is
            // the sibling of this executable.
            let mut words = vec![image.to_string_lossy().into_owned()];
            words.extend(args.iter().cloned());
            let parsed = parse_args(&words).expect("the helper's own parser accepts the arguments");
            let me = inspect_current().expect("own process");
            assert_eq!(parsed.ui_pid, me.identity.pid);
            assert_eq!(parsed.ui_created_at_100ns, me.identity.created_at_100ns);
            assert_eq!(
                image.file_name().and_then(|name| name.to_str()),
                Some(HELPER_IMAGE_NAME)
            );

            // The pipe exists before this returns, so the client can open it at once.
            let server = PipeServer::create(&parsed.pipe, &own_sid()).expect("pipe");
            let foreign_process = plan.foreign_process;
            let (memory, seen) = (Arc::clone(&self.memory), Arc::clone(&self.seen));
            lock(&self.servers).push(thread::spawn(move || {
                serve(&server, &plan, &memory, &seen);
            }));

            if foreign_process {
                let child = Command::new("cmd")
                    .args(["/C", "ping -n 20 127.0.0.1 > nul"])
                    .stdout(Stdio::null())
                    .spawn()
                    .expect("a stand-in process");
                let handle = ProcessHandle::open(child.id()).expect("its handle");
                lock(&self.children).push(child);
                Ok(handle)
            } else {
                Ok(ProcessHandle::open(std::process::id()).expect("own handle"))
            }
        }
    }

    /// An elevator over the replaced launch call. Availability is forced to `Available` and the
    /// user's own SID counts as a trusted pipe owner, because a test process is neither in
    /// Program Files nor elevated; the tests that need the real owner rule use `strict`.
    fn elevator(launcher: &TestLauncher) -> WinElevator {
        let mut elevator = WinElevator::build(hashes("a"), Box::new(launcher.clone()));
        elevator.test.status = Some(ElevationStatus::Available);
        elevator.test.trusted_owner = Some(own_sid());
        elevator
    }

    /// Like [`elevator`] but with the owner rule exactly as in a release build.
    fn strict(launcher: &TestLauncher) -> WinElevator {
        let mut elevator = WinElevator::build(hashes("a"), Box::new(launcher.clone()));
        elevator.test.status = Some(ElevationStatus::Available);
        elevator
    }

    fn purge(elevator: &WinElevator) -> Result<MemoryPurgeReply, ElevationError> {
        elevator.memory_purge(&[MemoryArea::StandbyList, MemoryArea::WorkingSets])
    }

    #[test]
    fn a_session_starts_one_helper_then_reuses_it_and_says_bye() {
        let launcher = TestLauncher::new(Mode::Serve(Plan::genuine()));
        let elevator = elevator(&launcher);

        let first = purge(&elevator).expect("purges");
        assert!(first.any_purged());
        assert!(matches!(first.areas[0].status, AreaStatus::Purged { .. }));
        assert_eq!(launcher.launches(), 1, "one prompt");
        let second = elevator
            .memory_purge(&[MemoryArea::ModifiedList])
            .expect("purges again");
        assert!(second.any_purged());
        assert_eq!(launcher.launches(), 1, "the second purge shows no prompt");
        assert_eq!(elevator.hello().expect("hello").catalogues, hashes("a"));

        elevator.bye();
        launcher.wait_for_helpers();
        let seen = launcher.seen();
        let ids: Vec<u64> = seen.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, [1, 2, 3, 4], "ids go 1, 2, 3, ...");
        assert!(matches!(
            seen[0].1,
            Request::Hello {
                protocol: PROTOCOL_VERSION
            }
        ));
        assert!(matches!(seen[3].1, Request::Bye {}));
        // Areas reached the helper once each, in the order given.
        assert_eq!(
            seen[1].1,
            Request::MemoryPurge {
                areas: vec![MemoryArea::StandbyList, MemoryArea::WorkingSets]
            }
        );

        // After bye the next request starts a new session (a new prompt).
        purge(&elevator).expect("a new session");
        assert_eq!(launcher.launches(), 2);
    }

    #[test]
    fn tweak_and_history_requests_share_the_session_and_carry_only_the_id() {
        let launcher = TestLauncher::new(Mode::Serve(Plan::genuine()));
        let elevator = elevator(&launcher);
        let applied = elevator.tweak_apply("some-tweak").expect("apply");
        assert_eq!(applied.outcome, volt_core::ApplyOutcome::Applied);
        assert!(
            elevator
                .tweak_revert("some-tweak")
                .expect("revert")
                .complete
        );
        assert_eq!(
            elevator.history(4, 10).expect("history").store,
            "test-store"
        );
        assert_eq!(launcher.launches(), 1, "one session, one prompt");
        elevator.bye();
        launcher.wait_for_helpers();
        let seen = launcher.seen();
        assert_eq!(
            seen[1].1,
            Request::TweakApply {
                tweak: "some-tweak".to_owned()
            }
        );
        assert_eq!(
            seen[2].1,
            Request::TweakRevert {
                tweak: "some-tweak".to_owned()
            }
        );
        assert_eq!(
            seen[3].1,
            Request::History {
                after: 4,
                limit: 10
            }
        );
    }

    #[test]
    fn a_pipe_served_by_another_process_than_the_one_started_is_refused_before_any_request() {
        let launcher = TestLauncher::new(Mode::Serve(Plan {
            foreign_process: true,
            ..Plan::genuine()
        }));
        let elevator = elevator(&launcher);
        assert!(matches!(
            purge(&elevator),
            Err(ElevationError::Untrusted(_))
        ));
        drop(elevator);
        launcher.wait_for_helpers();
        assert!(launcher.seen().is_empty(), "nothing was sent");
        assert!(launcher.memory.purged().is_empty());
    }

    #[test]
    fn a_pipe_with_an_untrusted_owner_is_refused_before_any_request() {
        // An elevated runner (CI) creates the pipe as Administrators, which IS trusted, so the
        // unelevated case cannot be built there.
        if current_token_kind() == Ok(TokenKind::Elevated) {
            return;
        }
        let launcher = TestLauncher::new(Mode::Serve(Plan::genuine()));
        let elevator = strict(&launcher);
        // This test process made the pipe, so its owner is the user.
        match purge(&elevator) {
            Err(ElevationError::Untrusted(why)) => assert!(why.contains("owned"), "{why}"),
            other => panic!("expected untrusted, got {other:?}"),
        }
        drop(elevator);
        launcher.wait_for_helpers();
        assert!(launcher.seen().is_empty(), "nothing was sent");
    }

    #[test]
    fn a_helper_with_other_catalogues_or_another_protocol_is_a_mismatch_and_is_dropped() {
        for plan in [
            Plan {
                hashes: hashes("b"),
                ..Plan::genuine()
            },
            Plan {
                protocol: PROTOCOL_VERSION + 1,
                ..Plan::genuine()
            },
        ] {
            let launcher = TestLauncher::new(Mode::Serve(plan));
            let elevator = elevator(&launcher);
            assert!(matches!(purge(&elevator), Err(ElevationError::Mismatch(_))));
            drop(elevator);
            launcher.wait_for_helpers();
            // Only the hello was ever sent, never the purge.
            let seen = launcher.seen();
            assert_eq!(seen.len(), 1);
            assert!(matches!(seen[0].1, Request::Hello { .. }));
            assert!(launcher.memory.purged().is_empty());
        }
    }

    #[test]
    fn a_declined_prompt_changes_nothing_and_is_not_repeated_by_itself() {
        let launcher = TestLauncher::new(Mode::Decline);
        let elevator = elevator(&launcher);
        assert_eq!(purge(&elevator), Err(ElevationError::Declined));
        assert_eq!(launcher.launches(), 1, "one click, one prompt");
        assert!(launcher.seen().is_empty());
        // The next click is a new request and may prompt again.
        assert_eq!(purge(&elevator), Err(ElevationError::Declined));
        assert_eq!(launcher.launches(), 2);
    }

    #[test]
    fn a_launch_that_fails_is_unavailable_with_the_reason() {
        let launcher = TestLauncher::new(Mode::Fail);
        let elevator = elevator(&launcher);
        match purge(&elevator) {
            Err(ElevationError::Unavailable(why)) => assert!(why.contains("blocked"), "{why}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn when_the_helper_has_exited_the_next_request_starts_a_new_one() {
        let launcher = TestLauncher::new(Mode::Serve(Plan {
            // hello and one purge, then the helper goes away.
            exit_after: Some(2),
            ..Plan::genuine()
        }));
        let elevator = elevator(&launcher);
        purge(&elevator).expect("the first purge");
        launcher.wait_for_helpers();
        assert_eq!(launcher.launches(), 1);

        // The helper is gone: this request starts a new one (one more prompt) and works.
        purge(&elevator).expect("the next purge");
        assert_eq!(launcher.launches(), 2);
        let seen = launcher.seen();
        // The new helper saw a fresh session: hello is id 1 again.
        assert_eq!(seen[2].0, 1);
        assert!(matches!(seen[2].1, Request::Hello { .. }));
    }

    #[test]
    fn a_helper_that_dies_in_the_middle_of_a_request_is_not_retried_by_itself() {
        let launcher = TestLauncher::new(Mode::Serve(Plan {
            // Dies on the second request, the purge, without answering.
            die_on: Some(2),
            ..Plan::genuine()
        }));
        let elevator = elevator(&launcher);
        assert_eq!(purge(&elevator), Err(ElevationError::Disconnected));
        assert_eq!(launcher.launches(), 1, "no second prompt by itself");
        launcher.wait_for_helpers();
        // The next user request starts over.
        let _ = purge(&elevator);
        assert_eq!(launcher.launches(), 2);
    }

    #[test]
    fn an_unavailable_status_never_launches_and_an_empty_request_never_prompts() {
        let launcher = TestLauncher::new(Mode::Serve(Plan::genuine()));
        let mut standard = WinElevator::build(hashes("a"), Box::new(launcher.clone()));
        standard.test.status = Some(ElevationStatus::Unavailable {
            reason: UnavailableReason::StandardUser,
        });
        assert_eq!(
            standard.status(),
            ElevationStatus::Unavailable {
                reason: UnavailableReason::StandardUser
            }
        );
        match purge(&standard) {
            Err(ElevationError::Unavailable(why)) => {
                assert!(why.contains("administrator"), "{why}");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(launcher.launches(), 0);

        let available = elevator(&launcher);
        assert_eq!(
            available.memory_purge(&[]),
            Err(ElevationError::Refused(ErrorCode::InvalidRequest))
        );
        assert_eq!(launcher.launches(), 0);
    }
}
