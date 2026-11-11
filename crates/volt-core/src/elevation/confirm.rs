//! The helper's confirmation, as a port, and the rules around it.
//!
//! A request with a persistent effect (an administrator tweak applied or reverted) is accepted by
//! the helper only after a person confirmed it on the helper's own surface, which a program of
//! the user's cannot click (see the decision on the helper). This module holds what does not
//! depend on how the question is shown:
//!
//! - [`Confirmer`], the port the helper asks. The real one draws a dialog; a build that has no
//!   dialog yet answers [`Verdict::Unavailable`] to everything, which fails closed;
//! - [`ConfirmGate`], the rules: after a denial the same request is refused for
//!   [`DENIAL_BLOCK`] without asking again, and after [`DENIALS_BEFORE_LOCKOUT`] denials every
//!   confirmed request is refused for the rest of the session. Time is passed in, so the rules are
//!   tested without waiting.
//!
//! The text of a question is built by the helper from its own catalogue ([`ConfirmRequest`] carries
//! the tweak's title and risk as the catalogue states them), never from anything the client sent
//! beyond the tweak id.

use std::time::{Duration, Instant};

use crate::label::Risk;

/// How long the same request stays refused after a denial.
pub const DENIAL_BLOCK: Duration = Duration::from_secs(10 * 60);

/// Denials in one session after which every confirmed request is refused.
pub const DENIALS_BEFORE_LOCKOUT: u32 = 2;

/// What is being confirmed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ConfirmAction {
    /// Turn an administrator tweak on.
    ApplyTweak,
    /// Revert what the helper applied for an administrator tweak.
    RevertTweak,
}

/// One question for the person at the machine. Everything in it comes from the helper's catalogue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmRequest {
    /// What is about to happen.
    pub action: ConfirmAction,
    /// The tweak id.
    pub tweak: String,
    /// The tweak's title as the catalogue states it.
    pub title: String,
    /// The tweak's risk label.
    pub risk: Risk,
}

/// The answer to a [`ConfirmRequest`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Verdict {
    /// A person accepted. The only answer that lets a change go ahead.
    Accepted,
    /// A person declined, or the question timed out unanswered.
    Denied,
    /// There is no way to ask (no dialog in this build, or it could not be shown). Treated as a
    /// refusal that is not a person's decision: it never counts towards the lockout.
    Unavailable,
}

/// Port: asks the person at the machine.
///
/// An implementation must fail closed: when it cannot be sure a person accepted, it answers
/// [`Verdict::Denied`] or [`Verdict::Unavailable`].
pub trait Confirmer: Send + Sync {
    /// Asks, and returns when the question is answered, timed out or could not be shown.
    fn confirm(&self, request: &ConfirmRequest) -> Verdict;
}

/// What the gate decided for one request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Decision {
    /// Accepted by a person: go ahead.
    Accepted,
    /// A person declined. The same request is now blocked for [`DENIAL_BLOCK`].
    Denied,
    /// Nobody could be asked.
    Unavailable,
    /// The same request was declined less than [`DENIAL_BLOCK`] ago: refused without asking.
    Blocked,
    /// [`DENIALS_BEFORE_LOCKOUT`] denials already: every confirmed request is refused without
    /// asking.
    LockedOut,
}

impl Decision {
    /// Whether the change may go ahead.
    #[must_use]
    pub fn is_accepted(self) -> bool {
        self == Self::Accepted
    }

    /// Whether this refusal was given without asking anyone.
    #[must_use]
    pub fn is_blocked(self) -> bool {
        matches!(self, Self::Blocked | Self::LockedOut)
    }

    /// The reason in words, for the activity log (English).
    #[must_use]
    pub fn reason(self) -> &'static str {
        match self {
            Self::Accepted => "confirmed",
            Self::Denied => "the confirmation was declined",
            Self::Unavailable => "no confirmation could be shown",
            Self::Blocked => "the same request was declined less than 10 minutes ago",
            Self::LockedOut => "confirmations are locked after repeated denials",
        }
    }
}

/// The denial rules of one helper session.
#[derive(Debug, Default)]
pub struct ConfirmGate {
    /// Denied requests and when.
    denied: Vec<(ConfirmAction, String, Instant)>,
    /// Denials so far in this session.
    denials: u32,
}

impl ConfirmGate {
    /// A gate that has seen no denial.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether a request would be refused without asking at `now`.
    #[must_use]
    pub fn refusal_without_asking(
        &self,
        action: ConfirmAction,
        tweak: &str,
        now: Instant,
    ) -> Option<Decision> {
        if self.denials >= DENIALS_BEFORE_LOCKOUT {
            return Some(Decision::LockedOut);
        }
        self.denied
            .iter()
            .any(|(denied, id, at)| {
                *denied == action
                    && id == tweak
                    && now.saturating_duration_since(*at) < DENIAL_BLOCK
            })
            .then_some(Decision::Blocked)
    }

    /// Decides one request: refuses it without asking when a denial rule says so, otherwise asks
    /// `confirmer` and records a denial. Only [`Decision::Accepted`] lets the change go ahead.
    pub fn decide(
        &mut self,
        confirmer: &dyn Confirmer,
        request: &ConfirmRequest,
        now: Instant,
    ) -> Decision {
        if let Some(refusal) = self.refusal_without_asking(request.action, &request.tweak, now) {
            return refusal;
        }
        match confirmer.confirm(request) {
            Verdict::Accepted => Decision::Accepted,
            Verdict::Denied => {
                self.denials += 1;
                self.denied
                    .push((request.action, request.tweak.clone(), now));
                Decision::Denied
            }
            // Anything that is not a clear acceptance is a refusal; a verdict this version does
            // not know is one too.
            _ => Decision::Unavailable,
        }
    }

    /// How many denials this session has seen.
    #[must_use]
    pub fn denials(&self) -> u32 {
        self.denials
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::elevation::fakes::FakeConfirmer;

    fn request(action: ConfirmAction, tweak: &str) -> ConfirmRequest {
        ConfirmRequest {
            action,
            tweak: tweak.to_owned(),
            title: "A tweak".to_owned(),
            risk: Risk::Safe,
        }
    }

    #[test]
    fn only_an_acceptance_lets_a_change_go_ahead() {
        let mut gate = ConfirmGate::new();
        let now = Instant::now();
        let ask = request(ConfirmAction::ApplyTweak, "a");
        assert_eq!(
            gate.decide(&FakeConfirmer::accepting(), &ask, now),
            Decision::Accepted
        );
        assert_eq!(
            gate.decide(&FakeConfirmer::unavailable(), &ask, now),
            Decision::Unavailable
        );
        assert_eq!(
            gate.decide(&FakeConfirmer::denying(), &ask, now),
            Decision::Denied
        );
        assert!(Decision::Accepted.is_accepted());
        for refusal in [
            Decision::Denied,
            Decision::Unavailable,
            Decision::Blocked,
            Decision::LockedOut,
        ] {
            assert!(!refusal.is_accepted(), "{refusal:?}");
        }
    }

    #[test]
    fn a_denial_blocks_the_same_request_for_ten_minutes_without_asking() {
        let mut gate = ConfirmGate::new();
        let confirmer = FakeConfirmer::denying();
        let start = Instant::now();
        let ask = request(ConfirmAction::ApplyTweak, "a");

        assert_eq!(gate.decide(&confirmer, &ask, start), Decision::Denied);
        assert_eq!(confirmer.asked().len(), 1);

        // Blocked up to the tenth minute, and nobody is asked.
        let accepting = FakeConfirmer::accepting();
        for minutes in [0, 1, 9] {
            let later = start + Duration::from_secs(minutes * 60 + 59);
            assert_eq!(
                gate.decide(&accepting, &ask, later),
                Decision::Blocked,
                "{minutes} min"
            );
        }
        assert!(accepting.asked().is_empty());

        // After ten minutes it is asked again.
        assert_eq!(
            gate.decide(&accepting, &ask, start + DENIAL_BLOCK),
            Decision::Accepted
        );
    }

    #[test]
    fn the_block_is_for_the_same_action_on_the_same_tweak() {
        let mut gate = ConfirmGate::new();
        let start = Instant::now();
        let deny = FakeConfirmer::denying();
        assert_eq!(
            gate.decide(&deny, &request(ConfirmAction::ApplyTweak, "a"), start),
            Decision::Denied
        );
        let accepting = FakeConfirmer::accepting();
        // Another tweak, and the other action on the same tweak, are asked.
        assert_eq!(
            gate.decide(&accepting, &request(ConfirmAction::ApplyTweak, "b"), start),
            Decision::Accepted
        );
        assert_eq!(
            gate.decide(&accepting, &request(ConfirmAction::RevertTweak, "a"), start),
            Decision::Accepted
        );
    }

    #[test]
    fn two_denials_lock_every_confirmed_request_for_the_session() {
        let mut gate = ConfirmGate::new();
        let start = Instant::now();
        let deny = FakeConfirmer::denying();
        for tweak in ["a", "b"] {
            assert_eq!(
                gate.decide(&deny, &request(ConfirmAction::ApplyTweak, tweak), start),
                Decision::Denied
            );
        }
        assert_eq!(gate.denials(), DENIALS_BEFORE_LOCKOUT);

        let accepting = FakeConfirmer::accepting();
        // A third tweak, and the first one long after its own block ended, are both refused.
        let much_later = start + Duration::from_secs(3 * 60 * 60);
        for (tweak, at) in [("c", start), ("a", much_later)] {
            assert_eq!(
                gate.decide(&accepting, &request(ConfirmAction::ApplyTweak, tweak), at),
                Decision::LockedOut
            );
        }
        assert!(accepting.asked().is_empty());
    }

    #[test]
    fn an_unavailable_confirmation_is_a_refusal_that_never_counts_as_a_denial() {
        let mut gate = ConfirmGate::new();
        let start = Instant::now();
        let none = FakeConfirmer::unavailable();
        for _ in 0..5 {
            assert_eq!(
                gate.decide(&none, &request(ConfirmAction::ApplyTweak, "a"), start),
                Decision::Unavailable
            );
        }
        assert_eq!(gate.denials(), 0);
        // So it neither blocks nor locks: a build that gets a dialog later is asked at once.
        assert_eq!(
            gate.decide(
                &FakeConfirmer::accepting(),
                &request(ConfirmAction::ApplyTweak, "a"),
                start
            ),
            Decision::Accepted
        );
    }

    #[test]
    fn a_decision_says_why_in_words() {
        for decision in [
            Decision::Accepted,
            Decision::Denied,
            Decision::Unavailable,
            Decision::Blocked,
            Decision::LockedOut,
        ] {
            assert!(!decision.reason().is_empty());
        }
        assert!(Decision::Blocked.is_blocked() && Decision::LockedOut.is_blocked());
        assert!(!Decision::Denied.is_blocked());
    }
}
