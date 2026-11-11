//! Refuses to start the UI while elevated.
//!
//! Volt's tray, core and `WebView` never run elevated by choice (an elevated `WebView` over a
//! user-writable profile is exactly what the helper design avoids). When the UI process itself is
//! started elevated and UAC is on, it says so in a native message box and exits before any window
//! or `WebView` exists. Where there is no boundary to protect (UAC switched off system-wide, or the
//! built-in Administrator account) it runs as usual; Settings tells the user Volt cannot limit
//! itself there.

use volt_win::helper::identity::{TokenKind, current_token_kind};
use volt_win::helper::uac;

use crate::language::{Lang, system_language};

/// What the process found out about itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Probes {
    /// The token of this process, or `None` when it could not be read.
    pub token: Option<TokenKind>,
    /// The system-wide UAC switch, or `None` when it could not be read.
    pub uac_enabled: Option<bool>,
    /// The user is the built-in Administrator account.
    pub built_in_administrator: bool,
}

/// What to do at start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartDecision {
    /// Not elevated: start normally.
    Run,
    /// Elevated with nothing to protect (UAC off, or the built-in Administrator): start normally.
    RunWithoutBoundary,
    /// Elevated while UAC is on: say so and exit.
    Refuse,
}

/// The decision over the probes. An elevated process whose UAC state cannot be read is refused
/// (the safe answer). A token that cannot be read counts as not elevated: that is far more likely
/// a failed probe than an elevated start, and a refusal would lock every user out of the app.
#[must_use]
pub const fn decide(probes: Probes) -> StartDecision {
    match probes.token {
        Some(TokenKind::Limited | TokenKind::Standard) | None => StartDecision::Run,
        Some(TokenKind::Elevated) => {
            if probes.built_in_administrator || matches!(probes.uac_enabled, Some(false)) {
                StartDecision::RunWithoutBoundary
            } else {
                StartDecision::Refuse
            }
        }
    }
}

/// The title and text of the message, in `lang`.
#[must_use]
pub const fn refusal_text(lang: Lang) -> (&'static str, &'static str) {
    match lang {
        Lang::English => (
            "Volt",
            "Volt does not run as administrator.\n\nStart Volt normally (not with \"Run as \
             administrator\"). When an action needs administrator rights, Volt asks for them \
             itself, once, and only for that action.",
        ),
        Lang::Vietnamese => (
            "Volt",
            "Volt không chạy với quyền quản trị.\n\nHãy mở Volt bình thường (không chọn \"Run as \
             administrator\"). Khi một thao tác cần quyền quản trị, Volt sẽ tự xin quyền đó, một \
             lần và chỉ cho thao tác đó.",
        ),
    }
}

/// The probes of this process.
fn probe() -> Probes {
    Probes {
        token: current_token_kind().ok(),
        uac_enabled: uac::uac_enabled(),
        built_in_administrator: uac::current_user_is_built_in_administrator(),
    }
}

/// Returns `true` when the UI may start. When it must not, shows the message and returns `false`;
/// the caller exits without creating any window.
#[must_use]
pub fn allow_start() -> bool {
    if decide(probe()) == StartDecision::Refuse {
        let (title, text) = refusal_text(system_language());
        uac::show_message(title, text);
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probes(token: Option<TokenKind>, uac: Option<bool>, built_in: bool) -> Probes {
        Probes {
            token,
            uac_enabled: uac,
            built_in_administrator: built_in,
        }
    }

    #[test]
    fn an_unelevated_start_always_runs() {
        for token in [TokenKind::Limited, TokenKind::Standard] {
            for uac in [Some(true), Some(false), None] {
                assert_eq!(
                    decide(probes(Some(token), uac, false)),
                    StartDecision::Run,
                    "{token:?} {uac:?}"
                );
            }
        }
    }

    #[test]
    fn an_elevated_start_with_uac_on_is_refused() {
        assert_eq!(
            decide(probes(Some(TokenKind::Elevated), Some(true), false)),
            StartDecision::Refuse
        );
    }

    #[test]
    fn an_elevated_start_with_an_unreadable_uac_state_is_refused() {
        assert_eq!(
            decide(probes(Some(TokenKind::Elevated), None, false)),
            StartDecision::Refuse
        );
    }

    #[test]
    fn with_uac_off_or_for_the_built_in_administrator_it_runs_without_a_boundary() {
        for (uac, built_in) in [(Some(false), false), (Some(true), true), (None, true)] {
            assert_eq!(
                decide(probes(Some(TokenKind::Elevated), uac, built_in)),
                StartDecision::RunWithoutBoundary,
                "{uac:?} {built_in}"
            );
        }
    }

    #[test]
    fn an_unreadable_token_does_not_stop_the_app() {
        assert_eq!(decide(probes(None, Some(true), false)), StartDecision::Run);
    }

    #[test]
    fn the_message_exists_in_both_languages_and_differs() {
        let (en_title, en) = refusal_text(Lang::English);
        let (vi_title, vi) = refusal_text(Lang::Vietnamese);
        assert_eq!(en_title, vi_title);
        assert_ne!(en, vi);
        assert!(en.contains("administrator") && vi.contains("quản trị"));
    }
}
