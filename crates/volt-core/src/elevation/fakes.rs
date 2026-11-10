//! In-memory fake of the [`Elevator`] port.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use super::{
    CatalogueHashes, ElevationError, Elevator, ErrorCode, HelloReply, MemoryPurgeReply,
    PROTOCOL_VERSION,
};
use crate::journal::Source;
use crate::memory::MemoryCleaner;
use crate::ports::{Memory, MemoryArea};
use crate::time::SystemClock;

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
    /// Administrator features are unavailable, with this reason.
    Unavailable(String),
}

/// A call the fake received.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ElevatorCall {
    /// `hello`.
    Hello,
    /// `memory_purge` with these areas.
    MemoryPurge(Vec<MemoryArea>),
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

    /// An elevator that is unavailable for `reason`.
    #[must_use]
    pub fn unavailable(memory: Arc<dyn Memory>, reason: &str) -> Self {
        Self::with(memory, Behaviour::Unavailable(reason.to_owned()))
    }

    /// Sets the catalogue hashes `hello` reports.
    pub fn set_catalogue_hashes(&self, hashes: CatalogueHashes) {
        lock(&self.state).hashes = hashes;
    }

    /// Sets the protocol version `hello` reports (to test a mismatch).
    pub fn set_protocol(&self, protocol: u32) {
        lock(&self.state).protocol = protocol;
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
            Behaviour::Unavailable(reason) => Err(ElevationError::Unavailable(reason.clone())),
        }
    }
}

impl Elevator for FakeElevator {
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

    fn bye(&self) {
        let mut state = lock(&self.state);
        state.calls.push(ElevatorCall::Bye);
        state.active = false;
    }
}
