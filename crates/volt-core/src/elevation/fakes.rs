//! In-memory fake of the [`Elevator`] port.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use super::confirm::{ConfirmRequest, Confirmer, Verdict};
use super::restore::{RestorePoints, SystemProtection};
use super::{
    CatalogueHashes, ElevationError, ElevationStatus, Elevator, ErrorCode, HelloReply,
    HistoryReply, MemoryPurgeReply, PROTOCOL_VERSION, TweakApplyReply, TweakRevertReply,
    UnavailableReason,
};
use crate::activity::ActivityRecord;
use crate::error::PortError;
use crate::journal::Source;
use crate::memory::MemoryCleaner;
use crate::ports::{Memory, MemoryArea};
use crate::time::SystemClock;
use crate::tweak::ApplyOutcome;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What the fake does when a session has to start.
#[derive(Debug, Clone)]
enum Behaviour {
    /// The user accepts the prompt.
    Grant,
    /// The user declines the prompt.
    Decline,
    /// Administrator features are unavailable, for this reason.
    Unavailable(UnavailableReason),
}

/// A call the fake received.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ElevatorCall {
    /// `status`.
    Status,
    /// `hello`.
    Hello,
    /// `memory_purge` with these areas.
    MemoryPurge(Vec<MemoryArea>),
    /// `tweak_apply` for this tweak id.
    TweakApply(String),
    /// `tweak_revert` for this tweak id.
    TweakRevert(String),
    /// `history` after this record id.
    History(u64),
    /// `bye`.
    Bye,
}

#[derive(Debug)]
struct State {
    behaviour: Behaviour,
    active: bool,
    prompts: u32,
    calls: Vec<ElevatorCall>,
    hashes: CatalogueHashes,
    protocol: u32,
    /// What `tweak_apply` and `tweak_revert` answer instead of success, when set.
    tweak_error: Option<ElevationError>,
    /// The helper's log: its store id and records, oldest first.
    history: (String, Vec<ActivityRecord>),
    /// What `system_protection` answers.
    system_protection: SystemProtection,
}

/// In-memory [`Elevator`]. A purge runs against the [`Memory`] port it was given (a
/// [`FakeMemory`](crate::fakes::FakeMemory) in tests), the way the real helper runs it against the
/// real one. Starting a session counts as one prompt, so a test can check that the second purge
/// of a session does not prompt again.
pub struct FakeElevator {
    memory: Arc<dyn Memory>,
    state: Mutex<State>,
}

impl std::fmt::Debug for FakeElevator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeElevator").finish_non_exhaustive()
    }
}

impl FakeElevator {
    fn with(memory: Arc<dyn Memory>, behaviour: Behaviour) -> Self {
        Self {
            memory,
            state: Mutex::new(State {
                behaviour,
                active: false,
                prompts: 0,
                calls: Vec::new(),
                hashes: CatalogueHashes {
                    tweaks: "fake-tweaks".to_owned(),
                    cleaning: "fake-cleaning".to_owned(),
                },
                protocol: PROTOCOL_VERSION,
                tweak_error: None,
                history: ("fake-store".to_owned(), Vec::new()),
                system_protection: SystemProtection::Unknown,
            }),
        }
    }

    /// An elevator whose user accepts the prompt; purges run on `memory`.
    #[must_use]
    pub fn granted(memory: Arc<dyn Memory>) -> Self {
        Self::with(memory, Behaviour::Grant)
    }

    /// An elevator whose user declines the prompt.
    #[must_use]
    pub fn declined(memory: Arc<dyn Memory>) -> Self {
        Self::with(memory, Behaviour::Decline)
    }

    /// Sets what the elevator answers to `system_protection`.
    pub fn set_system_protection(&self, value: SystemProtection) {
        lock(&self.state).system_protection = value;
    }

    /// An elevator that is unavailable for `reason`, given in words.
    #[must_use]
    pub fn unavailable(memory: Arc<dyn Memory>, reason: &str) -> Self {
        Self::unavailable_because(
            memory,
            UnavailableReason::Unknown {
                detail: reason.to_owned(),
            },
        )
    }

    /// An elevator that is unavailable for a typed `reason`.
    #[must_use]
    pub fn unavailable_because(memory: Arc<dyn Memory>, reason: UnavailableReason) -> Self {
        Self::with(memory, Behaviour::Unavailable(reason))
    }

    /// Sets the catalogue hashes `hello` reports.
    pub fn set_catalogue_hashes(&self, hashes: CatalogueHashes) {
        lock(&self.state).hashes = hashes;
    }

    /// Sets the protocol version `hello` reports (to test a mismatch).
    pub fn set_protocol(&self, protocol: u32) {
        lock(&self.state).protocol = protocol;
    }

    /// Makes `tweak_apply` and `tweak_revert` answer `error` (after the session started), or
    /// succeed again with `None`.
    pub fn set_tweak_error(&self, error: Option<ElevationError>) {
        lock(&self.state).tweak_error = error;
    }

    /// Sets the helper's activity log: its store id and records, oldest first.
    pub fn set_history(&self, store: &str, records: Vec<ActivityRecord>) {
        lock(&self.state).history = (store.to_owned(), records);
    }

    /// How many times a session was started, which is how many prompts the user saw.
    #[must_use]
    pub fn prompts(&self) -> u32 {
        lock(&self.state).prompts
    }

    /// Whether a session is active now.
    #[must_use]
    pub fn is_active(&self) -> bool {
        lock(&self.state).active
    }

    /// Every call so far, in order.
    #[must_use]
    pub fn calls(&self) -> Vec<ElevatorCall> {
        lock(&self.state).calls.clone()
    }

    /// Starts a session when none is active.
    fn ensure_session(state: &mut State) -> Result<(), ElevationError> {
        if state.active {
            return Ok(());
        }
        match &state.behaviour {
            Behaviour::Grant => {
                state.active = true;
                state.prompts += 1;
                Ok(())
            }
            Behaviour::Decline => {
                state.prompts += 1;
                Err(ElevationError::Declined)
            }
            Behaviour::Unavailable(reason) => Err(ElevationError::Unavailable(reason.to_string())),
        }
    }
}

impl Elevator for FakeElevator {
    fn status(&self) -> ElevationStatus {
        let mut state = lock(&self.state);
        state.calls.push(ElevatorCall::Status);
        match &state.behaviour {
            Behaviour::Unavailable(reason) => ElevationStatus::Unavailable {
                reason: reason.clone(),
            },
            Behaviour::Grant | Behaviour::Decline => ElevationStatus::Available,
        }
    }

    fn hello(&self) -> Result<HelloReply, ElevationError> {
        let mut state = lock(&self.state);
        state.calls.push(ElevatorCall::Hello);
        Self::ensure_session(&mut state)?;
        Ok(HelloReply {
            protocol: state.protocol,
            helper_version: "fake".to_owned(),
            catalogues: state.hashes.clone(),
        })
    }

    fn memory_purge(&self, areas: &[MemoryArea]) -> Result<MemoryPurgeReply, ElevationError> {
        let mut state = lock(&self.state);
        state.calls.push(ElevatorCall::MemoryPurge(areas.to_vec()));
        Self::ensure_session(&mut state)?;
        drop(state);
        if areas.is_empty() {
            return Err(ElevationError::Refused(ErrorCode::InvalidRequest));
        }
        let cleaner = MemoryCleaner::new(Arc::clone(&self.memory), Arc::new(SystemClock));
        Ok(MemoryPurgeReply::from(&cleaner.run(areas, Source::Manual)))
    }

    fn tweak_apply(&self, tweak: &str) -> Result<TweakApplyReply, ElevationError> {
        let mut state = lock(&self.state);
        state.calls.push(ElevatorCall::TweakApply(tweak.to_owned()));
        Self::ensure_session(&mut state)?;
        if let Some(error) = state.tweak_error.clone() {
            return Err(error);
        }
        Ok(TweakApplyReply {
            tweak: tweak.to_owned(),
            outcome: ApplyOutcome::Applied,
        })
    }

    fn tweak_revert(&self, tweak: &str) -> Result<TweakRevertReply, ElevationError> {
        let mut state = lock(&self.state);
        state
            .calls
            .push(ElevatorCall::TweakRevert(tweak.to_owned()));
        Self::ensure_session(&mut state)?;
        if let Some(error) = state.tweak_error.clone() {
            return Err(error);
        }
        Ok(TweakRevertReply {
            tweak: tweak.to_owned(),
            batches: 1,
            complete: true,
        })
    }

    fn history(&self, after: u64, limit: u32) -> Result<HistoryReply, ElevationError> {
        let mut state = lock(&self.state);
        state.calls.push(ElevatorCall::History(after));
        Self::ensure_session(&mut state)?;
        let (store, all) = &state.history;
        let wanted = usize::try_from(limit.min(super::HISTORY_MAX_RECORDS)).unwrap_or(0);
        let mut records: Vec<ActivityRecord> = all
            .iter()
            .filter(|record| record.id.0 > after)
            .take(wanted + 1)
            .cloned()
            .collect();
        let more = records.len() > wanted;
        records.truncate(wanted);
        Ok(HistoryReply {
            store: store.clone(),
            records,
            more,
        })
    }

    fn system_protection(&self) -> SystemProtection {
        lock(&self.state).system_protection
    }

    fn session_open(&self) -> bool {
        lock(&self.state).active
    }

    fn bye(&self) {
        let mut state = lock(&self.state);
        state.calls.push(ElevatorCall::Bye);
        state.active = false;
    }
}

/// A [`Confirmer`] that answers from a script and remembers what it was asked.
///
/// The answers are used in order; when they run out the last one repeats. The harness of the
/// elevated CI job has its own accepting confirmer in the helper crate, compiled only with that
/// job's feature: this fake is for tests and is not part of any product build.
pub struct FakeConfirmer {
    script: Mutex<(Vec<Verdict>, usize)>,
    asked: Mutex<Vec<ConfirmRequest>>,
}

impl std::fmt::Debug for FakeConfirmer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeConfirmer").finish_non_exhaustive()
    }
}

impl FakeConfirmer {
    /// Answers with `script` in order, then repeats the last answer. An empty script is
    /// [`Verdict::Unavailable`].
    #[must_use]
    pub fn scripted(script: Vec<Verdict>) -> Self {
        Self {
            script: Mutex::new((script, 0)),
            asked: Mutex::new(Vec::new()),
        }
    }

    /// A confirmer whose person always accepts.
    #[must_use]
    pub fn accepting() -> Self {
        Self::scripted(vec![Verdict::Accepted])
    }

    /// A confirmer whose person always declines.
    #[must_use]
    pub fn denying() -> Self {
        Self::scripted(vec![Verdict::Denied])
    }

    /// A confirmer that cannot ask anyone.
    #[must_use]
    pub fn unavailable() -> Self {
        Self::scripted(vec![Verdict::Unavailable])
    }

    /// Every question so far, in order.
    #[must_use]
    pub fn asked(&self) -> Vec<ConfirmRequest> {
        lock(&self.asked).clone()
    }
}

impl Confirmer for FakeConfirmer {
    fn confirm(&self, request: &ConfirmRequest) -> Verdict {
        lock(&self.asked).push(request.clone());
        let mut script = lock(&self.script);
        let (answers, next) = &mut *script;
        let verdict = answers
            .get(*next)
            .or_else(|| answers.last())
            .copied()
            .unwrap_or(Verdict::Unavailable);
        *next += 1;
        verdict
    }
}

/// A [`RestorePoints`] that remembers what it was asked and can be made to fail.
#[derive(Debug, Default)]
pub struct FakeRestorePoints {
    fail: bool,
    descriptions: Mutex<Vec<String>>,
    attempts: Mutex<u32>,
}

impl FakeRestorePoints {
    /// A port whose restore points are created.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A port that cannot create a restore point (System Protection is off, say).
    #[must_use]
    pub fn failing() -> Self {
        Self {
            fail: true,
            ..Self::default()
        }
    }

    /// The descriptions of the points that were created, in order.
    #[must_use]
    pub fn descriptions(&self) -> Vec<String> {
        lock(&self.descriptions).clone()
    }

    /// How many times a point was asked for.
    #[must_use]
    pub fn attempts(&self) -> u32 {
        *lock(&self.attempts)
    }
}

impl RestorePoints for FakeRestorePoints {
    fn create(&self, description: &str) -> Result<(), PortError> {
        *lock(&self.attempts) += 1;
        if self.fail {
            return Err(PortError::Unsupported(
                "System Protection is off".to_owned(),
            ));
        }
        lock(&self.descriptions).push(description.to_owned());
        Ok(())
    }
}
