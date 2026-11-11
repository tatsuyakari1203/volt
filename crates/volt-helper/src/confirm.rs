//! The confirmer of this build.
//!
//! A change that persists is made only after a person confirms it on a surface of the helper's own
//! (see the decision on the helper). This part of the work has the question and the rules around
//! it ([`volt_core::Confirmer`], [`volt_core::ConfirmGate`]) but not yet the surface: the dialog on
//! a private desktop comes with the next part. Until then the real helper's confirmer is
//! [`RefusingConfirmer`], which answers "cannot ask" to every question, so **every persistent
//! request is refused and nothing is ever changed**. It fails closed on purpose.
//!
//! Nothing turns the refusal off: there is no environment variable, argument or setting that is
//! read, and no cargo feature of the product build. The only other confirmer in this crate is the
//! accepting one of the elevated CI job's harness, which is compiled only with the `elevated-ci`
//! feature (which a release build refuses, see `lib.rs`, and which no workflow of the product
//! enables) and which a CI step proves is absent from the release helper.

use std::sync::Arc;

use volt_core::{ConfirmRequest, Confirmer, Verdict};

/// Answers [`Verdict::Unavailable`] to every question. See the module documentation.
#[derive(Debug, Default, Clone, Copy)]
pub struct RefusingConfirmer;

impl Confirmer for RefusingConfirmer {
    fn confirm(&self, _request: &ConfirmRequest) -> Verdict {
        Verdict::Unavailable
    }
}

/// The confirmer the helper binary uses: the refusing one in every product build.
#[must_use]
pub fn product_confirmer() -> Arc<dyn Confirmer> {
    #[cfg(feature = "elevated-ci")]
    {
        Arc::new(harness::AcceptingConfirmer)
    }
    #[cfg(not(feature = "elevated-ci"))]
    {
        Arc::new(RefusingConfirmer)
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
    use volt_core::{ConfirmAction, Risk};

    use super::*;

    #[test]
    fn the_refusing_confirmer_cannot_be_talked_into_accepting() {
        let question = |action| ConfirmRequest {
            action,
            tweak: "turn-off-fast-startup".to_owned(),
            title: "Turn off fast startup".to_owned(),
            risk: Risk::Safe,
        };
        for action in [ConfirmAction::ApplyTweak, ConfirmAction::RevertTweak] {
            assert_eq!(
                RefusingConfirmer.confirm(&question(action)),
                Verdict::Unavailable
            );
        }
    }

    #[cfg(not(feature = "elevated-ci"))]
    #[test]
    fn a_product_build_has_only_the_refusing_confirmer() {
        let question = ConfirmRequest {
            action: ConfirmAction::ApplyTweak,
            tweak: "x".to_owned(),
            title: "X".to_owned(),
            risk: Risk::Safe,
        };
        assert_eq!(product_confirmer().confirm(&question), Verdict::Unavailable);
    }
}
