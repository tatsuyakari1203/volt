//! The confirmer of this build.
//!
//! A change that persists is made only after a person confirms it on a surface of the helper's own
//! (see the decision on the helper). The helper's confirmer is [`DesktopConfirmer`]: it writes the
//! question from the helper's catalogue ([`crate::prompt_text`]) and shows it on a private desktop
//! that only Administrators can open ([`volt_win::helper::prompt`]). The rules around the question
//! (a denial blocks the same request for ten minutes, two denials lock the session) are
//! [`volt_core::ConfirmGate`]'s.
//!
//! The mapping to a verdict fails closed:
//!
//! - a click on the accept button is [`Verdict::Accepted`], and nothing else is;
//! - Cancel, Escape, closing the window and a question nobody answered in time are
//!   [`Verdict::Denied`] (a timeout counts as a denial);
//! - a desktop that cannot be made or switched to, or a session that cannot be switched back (it is
//!   locked), is [`Verdict::Unavailable`]: the request is refused and nothing is shown.
//!
//! Nothing turns the question off: there is no environment variable, argument or setting that is
//! read, and no cargo feature of the product build. The only other confirmer in this crate is the
//! accepting one of the elevated CI job's harness, which is compiled only with the `elevated-ci`
//! feature (which a release build refuses, see `lib.rs`, and which no workflow of the product
//! enables) and which a CI step proves is absent from the release helper.

use std::sync::Arc;
use std::time::Duration;

use volt_core::{ConfirmRequest, Confirmer, Verdict};
use volt_win::helper::prompt::{self, PromptAnswer};

use crate::prompt_text::{Lang, prompt_text};

/// How long the question waits for an answer in the product helper. Shorter than the UI's wait for
/// a tweak request (120 s), so the helper has answered before the UI gives up.
pub const PRODUCT_TIMEOUT: Duration = Duration::from_secs(45);

/// Asks on a private desktop. See the module documentation.
#[derive(Debug, Clone, Copy)]
pub struct DesktopConfirmer {
    timeout: Duration,
}

impl DesktopConfirmer {
    /// A confirmer that waits `timeout` for an answer. The product uses [`PRODUCT_TIMEOUT`]; the
    /// elevated CI job's harness uses a short one so a question nobody answers ends quickly.
    #[must_use]
    pub const fn with_timeout(timeout: Duration) -> Self {
        Self { timeout }
    }
}

/// The verdict for how a question ended.
#[must_use]
pub fn verdict_of(answer: &PromptAnswer) -> Verdict {
    match answer {
        PromptAnswer::Accepted => Verdict::Accepted,
        PromptAnswer::Declined | PromptAnswer::TimedOut => Verdict::Denied,
        // `PromptAnswer` is non-exhaustive: an answer this build does not know is not an
        // acceptance.
        _ => Verdict::Unavailable,
    }
}

impl Confirmer for DesktopConfirmer {
    fn confirm(&self, request: &ConfirmRequest) -> Verdict {
        let text = prompt_text(Lang::of_windows(), request, self.timeout);
        let answer = prompt::ask(&text, self.timeout);
        if let PromptAnswer::Unavailable(reason) = &answer {
            log::warn!("the confirmation could not be shown: {reason}");
        }
        verdict_of(&answer)
    }
}

/// The confirmer the helper binary uses: the private-desktop one in every product build.
#[must_use]
pub fn product_confirmer() -> Arc<dyn Confirmer> {
    #[cfg(feature = "elevated-ci")]
    {
        Arc::new(harness::AcceptingConfirmer)
    }
    #[cfg(not(feature = "elevated-ci"))]
    {
        Arc::new(DesktopConfirmer::with_timeout(PRODUCT_TIMEOUT))
    }
}

/// What exists only in the harness build of the elevated CI job.
#[cfg(feature = "elevated-ci")]
pub mod harness {
    use volt_core::{ConfirmRequest, Confirmer, Verdict};

    /// A string that is in the binary only when the accepting confirmer is: the CI step that
    /// checks the release helper searches for it.
    pub const MARKER: &str = "volt-ci-accepting-confirmer";

    /// Accepts every question without asking anyone. Exists only with the `elevated-ci` feature,
    /// for the harness's apply and revert of its own test tweak on a throwaway virtual machine.
    #[derive(Debug, Default, Clone, Copy)]
    pub struct AcceptingConfirmer;

    impl Confirmer for AcceptingConfirmer {
        fn confirm(&self, request: &ConfirmRequest) -> Verdict {
            log::warn!("{MARKER}: accepting `{}` without asking", request.tweak);
            Verdict::Accepted
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_click_on_accept_is_an_acceptance() {
        assert_eq!(verdict_of(&PromptAnswer::Accepted), Verdict::Accepted);
        assert_eq!(verdict_of(&PromptAnswer::Declined), Verdict::Denied);
        assert_eq!(verdict_of(&PromptAnswer::TimedOut), Verdict::Denied);
        assert_eq!(
            verdict_of(&PromptAnswer::Unavailable("no desktop".to_owned())),
            Verdict::Unavailable
        );
    }

    #[test]
    fn the_product_wait_is_shorter_than_the_ui_wait_for_a_tweak_request() {
        // The UI gives a tweak request 120 s; the question, the restore point and the change all
        // have to fit inside it.
        assert!(PRODUCT_TIMEOUT <= Duration::from_secs(60));
    }
}
