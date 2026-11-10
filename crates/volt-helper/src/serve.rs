//! The serve loop: one pipe, one bound client, three clocks.
//!
//! [`Helper::bind`] creates the pipe; [`Helper::serve`] then
//!
//! 1. waits for a connection and checks the client (the bound UI process, same user, same session,
//!    Medium integrity or above). Any other client is disconnected, counted in
//!    [`ServeReport::refused_clients`], and the helper keeps listening;
//! 2. serves the bound client's requests through a [`Session`], one message at a time;
//! 3. stops, and with it the whole helper, when that client says `bye`, closes the pipe or exits,
//!    when the helper has gone [`Limits::idle`] without a valid request (a refused request or a
//!    refused client never extends this), or when [`Limits::hard_cap`] has passed since it started.
//!
//! All waiting is a pipe call with a timeout, so there is no watchdog thread and nothing that has
//! to be killed from outside: the clocks are just the timeouts of the calls.

use std::time::{Duration, Instant};

use volt_core::MAX_REQUEST_BYTES;
use volt_win::helper::args::PipeName;
use volt_win::helper::identity::{
    ClientRefusal, ProcessIdentity, TokenFacts, check_client, inspect_current, inspect_process,
};
use volt_win::helper::pipe::{IoError, PipeServer};

use crate::session::Session;

/// The helper's clocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Exit after this long without a valid request.
    pub idle: Duration,
    /// Exit this long after the start, whatever happens.
    pub hard_cap: Duration,
    /// The longest a reply may take to be taken by the client.
    pub write_timeout: Duration,
    /// How long the helper waits for the client to read the last reply and close.
    pub close_grace: Duration,
}

impl Default for Limits {
    /// 10 idle minutes, 60 minutes at most.
    fn default() -> Self {
        Self {
            idle: Duration::from_secs(10 * 60),
            hard_cap: Duration::from_secs(60 * 60),
            write_timeout: Duration::from_secs(30),
            close_grace: Duration::from_secs(2),
        }
    }
}

/// Why the helper stopped serving.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExitReason {
    /// The bound client sent `bye`.
    Bye,
    /// The bound client closed the pipe (or its process ended).
    ClientDisconnected,
    /// No valid request for [`Limits::idle`].
    Idle,
    /// [`Limits::hard_cap`] passed.
    HardCap,
    /// The bound client broke the protocol (framing or id order) and was hung up on.
    ProtocolViolation,
    /// The pipe failed in a way that is not the client's doing.
    Failed(String),
}

/// What a serve run did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServeReport {
    /// Why it stopped.
    pub exit: ExitReason,
    /// Connections that were not the bound client and were disconnected.
    pub refused_clients: u32,
    /// Valid requests carried out.
    pub served_requests: u32,
}

/// A bound helper: the pipe exists and the client it will serve is fixed.
#[derive(Debug)]
pub struct Helper {
    server: PipeServer,
    own: TokenFacts,
    bound: ProcessIdentity,
}

impl Helper {
    /// Creates the pipe `pipe` for this process's user, to serve the `bound` process only.
    ///
    /// # Errors
    /// A message when the process's own token cannot be read or the pipe cannot be created (the
    /// name is taken, for example).
    pub fn bind(pipe: &PipeName, bound: ProcessIdentity) -> Result<Self, String> {
        let own = inspect_current()?.token;
        let server = PipeServer::create(pipe, &own.user_sid)?;
        Ok(Self { server, own, bound })
    }

    /// Whether the connected client is the one to serve.
    fn check_connected_client(&self) -> Result<(), ClientRefusal> {
        let pid = self
            .server
            .client_pid()
            .map_err(ClientRefusal::Unreadable)?;
        let facts = inspect_process(pid).map_err(ClientRefusal::Unreadable)?;
        check_client(&facts, &self.own, &self.bound)
    }

    /// Serves until a stop condition (see the module documentation). `session` carries the
    /// requests' behaviour; `limits` the clocks.
    #[must_use]
    pub fn serve(&self, mut session: Session, limits: &Limits) -> ServeReport {
        let started = Instant::now();
        let hard_deadline = started + limits.hard_cap;
        let mut last_valid = started;
        let mut report = ServeReport {
            exit: ExitReason::Idle,
            refused_clients: 0,
            served_requests: 0,
        };

        loop {
            let (deadline, expired) = next_deadline(last_valid, limits, hard_deadline);
            match self.server.accept(Some(remaining(deadline))) {
                Ok(()) => {}
                Err(IoError::Timeout) => {
                    report.exit = expired;
                    return report;
                }
                // A client came and went before it could be looked at: keep listening.
                Err(IoError::Closed) => {
                    self.server.disconnect();
                    continue;
                }
                Err(other) => {
                    report.exit = ExitReason::Failed(other.to_string());
                    return report;
                }
            }

            if self.check_connected_client().is_err() {
                // Not the bound UI: hang up on it, count it, keep listening. It is told nothing.
                self.server.disconnect();
                report.refused_clients += 1;
                continue;
            }

            report.exit = self.serve_connection(
                &mut session,
                limits,
                hard_deadline,
                &mut last_valid,
                &mut report.served_requests,
            );
            return report;
        }
    }

    /// Serves the bound client until the connection or a clock ends it.
    fn serve_connection(
        &self,
        session: &mut Session,
        limits: &Limits,
        hard_deadline: Instant,
        last_valid: &mut Instant,
        served: &mut u32,
    ) -> ExitReason {
        loop {
            let (deadline, expired) = next_deadline(*last_valid, limits, hard_deadline);
            let message = match self
                .server
                .read_message(MAX_REQUEST_BYTES, Some(remaining(deadline)))
            {
                Ok(message) => message,
                Err(IoError::Timeout) => return expired,
                Err(IoError::Closed) => return ExitReason::ClientDisconnected,
                Err(IoError::TooLarge) => {
                    return self.refuse_and_close(&Session::too_large().reply, limits);
                }
                Err(IoError::Os(error)) => return ExitReason::Failed(error),
            };

            let handled = session.handle(&message, Instant::now());
            if self
                .server
                .write_message(&handled.reply, Some(limits.write_timeout))
                .is_err()
            {
                return ExitReason::ClientDisconnected;
            }
            if handled.accepted {
                *last_valid = Instant::now();
                *served += 1;
            }
            if handled.close {
                self.server.finish(limits.close_grace);
                return if handled.accepted {
                    ExitReason::Bye
                } else {
                    ExitReason::ProtocolViolation
                };
            }
        }
    }

    /// Sends a last reply to a client that broke the framing and hangs up.
    fn refuse_and_close(&self, reply: &[u8], limits: &Limits) -> ExitReason {
        let _ = self.server.write_message(reply, Some(limits.write_timeout));
        self.server.finish(limits.close_grace);
        ExitReason::ProtocolViolation
    }
}

/// The next time something must stop the helper, and which clock it is.
fn next_deadline(
    last_valid: Instant,
    limits: &Limits,
    hard_deadline: Instant,
) -> (Instant, ExitReason) {
    let idle_deadline = last_valid + limits.idle;
    if idle_deadline <= hard_deadline {
        (idle_deadline, ExitReason::Idle)
    } else {
        (hard_deadline, ExitReason::HardCap)
    }
}

fn remaining(deadline: Instant) -> Duration {
    deadline.saturating_duration_since(Instant::now())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_idle_clock_wins_until_the_hard_cap_is_nearer() {
        let limits = Limits {
            idle: Duration::from_secs(10),
            hard_cap: Duration::from_secs(60),
            ..Limits::default()
        };
        let start = Instant::now();
        let hard = start + limits.hard_cap;
        assert_eq!(
            next_deadline(start, &limits, hard),
            (start + Duration::from_secs(10), ExitReason::Idle)
        );
        let late = start + Duration::from_secs(55);
        assert_eq!(
            next_deadline(late, &limits, hard),
            (hard, ExitReason::HardCap)
        );
    }

    #[test]
    fn the_default_clocks_are_the_decisions() {
        let limits = Limits::default();
        assert_eq!(limits.idle, Duration::from_secs(600));
        assert_eq!(limits.hard_cap, Duration::from_secs(3600));
    }
}
