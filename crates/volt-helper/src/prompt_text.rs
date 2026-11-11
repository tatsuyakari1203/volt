//! The words of the confirmation, in English and Vietnamese.
//!
//! The helper writes the question itself, from text embedded in the helper: nothing the client sent
//! beyond the tweak id ever reaches it, and the tweak's name comes from the helper's own
//! catalogue (English) or from the Vietnamese catalogue file embedded next to it.
//!
//! The language is the MACHINE's display language (`GetSystemDefaultUILanguage`), not the signed-in
//! user's: the user's display language is a setting in their registry hive, and no user-hive setting
//! may reach the elevated helper (decision on the helper, amendment 5). The machine value cannot be
//! changed without administrator rights, and all it does is choose between the two texts embedded
//! in this file, so nothing a user-level program does can put another text in front of the person.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;

use volt_core::{ConfirmAction, ConfirmRequest, Risk};
use volt_win::helper::prompt::PromptText;

/// The Vietnamese names of the tweaks, filed under `tweak.<id>.title` (the same file the UI uses).
const VI_CATALOGUE: &str =
    include_str!("../../../apps/desktop/src/shared/i18n/locales/vi.catalogue.json");

/// A language the question is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    /// English, the source language and the fallback.
    English,
    /// Vietnamese.
    Vietnamese,
}

impl Lang {
    /// The language of a Windows language id: Vietnamese for Vietnamese in any region, English for
    /// everything else.
    #[must_use]
    pub fn from_language_id(language_id: u16) -> Self {
        if volt_win::primary_language(language_id) == volt_win::LANG_VIETNAMESE {
            Self::Vietnamese
        } else {
            Self::English
        }
    }

    /// The machine's display language, English when it is not Vietnamese.
    #[must_use]
    pub fn of_machine() -> Self {
        Self::from_language_id(volt_win::machine_ui_language_id())
    }
}

fn vietnamese_titles() -> &'static HashMap<String, String> {
    static TITLES: OnceLock<HashMap<String, String>> = OnceLock::new();
    TITLES.get_or_init(|| {
        serde_json::from_str::<HashMap<String, String>>(VI_CATALOGUE).unwrap_or_default()
    })
}

/// The name of the tweak or cleaning rule in `lang`: the Vietnamese title when there is one, the
/// catalogue's English title otherwise.
#[must_use]
pub fn tweak_name(lang: Lang, request: &ConfirmRequest) -> String {
    let kind = if request.action == ConfirmAction::CleanRule {
        "rule"
    } else {
        "tweak"
    };
    if lang == Lang::Vietnamese
        && let Some(title) = vietnamese_titles().get(&format!("{kind}.{}.title", request.tweak))
    {
        return title.clone();
    }
    request.title.clone()
}

fn risk_word(lang: Lang, risk: Risk) -> &'static str {
    match (lang, risk) {
        (Lang::English, Risk::Safe) => "Safe",
        (Lang::English, Risk::Caution) => "Caution",
        (Lang::Vietnamese, Risk::Safe) => "An toàn",
        (Lang::Vietnamese, Risk::Caution) => "Thận trọng",
        (Lang::Vietnamese, _) => "Nâng cao",
        (Lang::English, _) => "Advanced",
    }
}

/// The question for `request`, in `lang`, that waits `timeout` for an answer.
#[must_use]
pub fn prompt_text(lang: Lang, request: &ConfirmRequest, timeout: Duration) -> PromptText {
    let name = tweak_name(lang, request);
    let risk = risk_word(lang, request.risk);
    let seconds = timeout.as_secs();
    if request.action == ConfirmAction::CleanRule {
        return clean_text(lang, &name, risk, seconds);
    }
    let apply = request.action == ConfirmAction::ApplyTweak;
    let restore = apply && volt_core::needs_restore_point(request.risk);
    match lang {
        Lang::English => {
            let verb = if apply { "Turn on" } else { "Turn off" };
            let mut body = if apply {
                format!(
                    "This changes a Windows setting for every account on this PC. Risk: {risk}."
                )
            } else {
                format!(
                    "This puts the setting back to what it was before Volt changed it, for every \
                     account on this PC. Risk: {risk}."
                )
            };
            if restore {
                body.push_str(" Windows creates a restore point first.");
            }
            body.push_str("\n\nContinue only if you just asked Volt to do this.");
            PromptText {
                title: "Volt: administrator confirmation".to_owned(),
                heading: format!("{verb} \"{name}\"?"),
                body,
                footer: format!("If nobody answers within {seconds} seconds, nothing is changed."),
                accept: verb.to_owned(),
                cancel: "Cancel".to_owned(),
            }
        }
        Lang::Vietnamese => {
            let verb = if apply { "Bật" } else { "Tắt" };
            let mut body = if apply {
                format!(
                    "Việc này thay đổi một cài đặt Windows cho mọi tài khoản trên máy này. Mức \
                     rủi ro: {risk}."
                )
            } else {
                format!(
                    "Việc này đặt cài đặt về giá trị trước khi Volt thay đổi, cho mọi tài khoản \
                     trên máy này. Mức rủi ro: {risk}."
                )
            };
            if restore {
                body.push_str(" Windows sẽ tạo một điểm khôi phục trước.");
            }
            body.push_str("\n\nChỉ tiếp tục nếu bạn vừa yêu cầu Volt làm việc này.");
            PromptText {
                title: "Volt: xác nhận quyền quản trị".to_owned(),
                heading: format!("{verb} \"{name}\"?"),
                body,
                footer: format!(
                    "Nếu không ai trả lời trong {seconds} giây, sẽ không có gì thay đổi."
                ),
                accept: verb.to_owned(),
                cancel: "Hủy".to_owned(),
            }
        }
    }
}

/// The question for a machine cleaning rule: what is deleted, that it is permanent and from where.
fn clean_text(lang: Lang, name: &str, risk: &str, seconds: u64) -> PromptText {
    match lang {
        Lang::English => PromptText {
            title: "Volt: administrator confirmation".to_owned(),
            heading: format!("Delete \"{name}\" permanently?"),
            body: format!(
                "This deletes old files from a folder that every account on this PC can put files in. \
                 They do not go to the Recycle Bin and cannot be restored. Risk: {risk}.\n\n\
                 Continue only if you just asked Volt to do this."
            ),
            footer: format!("If nobody answers within {seconds} seconds, nothing is deleted."),
            accept: "Delete permanently".to_owned(),
            cancel: "Cancel".to_owned(),
        },
        Lang::Vietnamese => PromptText {
            title: "Volt: xác nhận quyền quản trị".to_owned(),
            heading: format!("Xóa vĩnh viễn \"{name}\"?"),
            body: format!(
                "Việc này xóa các tệp cũ trong một thư mục mà mọi tài khoản trên máy này đều có thể \
                 đặt tệp vào. Chúng không vào Thùng rác và không thể khôi phục. Mức rủi ro: {risk}.\n\n\
                 Chỉ tiếp tục nếu bạn vừa yêu cầu Volt làm việc này."
            ),
            footer: format!("Nếu không ai trả lời trong {seconds} giây, sẽ không xóa gì cả."),
            accept: "Xóa vĩnh viễn".to_owned(),
            cancel: "Hủy".to_owned(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(action: ConfirmAction, risk: Risk) -> ConfirmRequest {
        ConfirmRequest {
            action,
            tweak: "turn-off-error-reporting".to_owned(),
            title: "Turn off Windows Error Reporting".to_owned(),
            risk,
        }
    }

    #[test]
    fn the_language_follows_the_machine_language_id() {
        assert_eq!(Lang::from_language_id(0x042A), Lang::Vietnamese);
        assert_eq!(Lang::from_language_id(0x2A), Lang::Vietnamese);
        assert_eq!(Lang::from_language_id(0x0409), Lang::English);
        assert_eq!(Lang::from_language_id(0x040C), Lang::English);
        assert_eq!(Lang::from_language_id(0), Lang::English);
    }

    #[test]
    fn the_helper_never_reads_the_users_display_language() {
        // The user's display language is a user-hive setting: only the machine's may be read here.
        let needles = [
            concat!("display", "_languages"),
            concat!("GetUserPreferred", "UILanguages"),
            concat!("GetUserDefault", "UILanguage"),
        ];
        for (file, text) in [
            ("prompt_text.rs", include_str!("prompt_text.rs")),
            ("confirm.rs", include_str!("confirm.rs")),
        ] {
            for needle in needles {
                assert!(!text.contains(needle), "{file} mentions {needle}");
            }
        }
    }

    #[test]
    fn the_question_names_the_tweak_in_the_language_of_the_person() {
        let ask = request(ConfirmAction::ApplyTweak, Risk::Safe);
        let english = prompt_text(Lang::English, &ask, Duration::from_secs(45));
        assert!(english.heading.contains("Turn off Windows Error Reporting"));
        assert_eq!(english.accept, "Turn on");
        let vietnamese = prompt_text(Lang::Vietnamese, &ask, Duration::from_secs(45));
        assert!(vietnamese.heading.contains("Tắt Windows Error Reporting"));
        assert_eq!(vietnamese.accept, "Bật");
        assert_eq!(vietnamese.cancel, "Hủy");
        assert!(vietnamese.footer.contains("45"));
    }

    #[test]
    fn every_administrator_tweak_has_a_vietnamese_name() {
        let catalogue =
            volt_core::Catalogue::from_toml(include_str!("../../../catalogue/tweaks.toml"))
                .expect("the catalogue loads");
        for tweak in catalogue
            .tweaks()
            .iter()
            .filter(|tweak| tweak.requires_admin)
        {
            assert!(
                vietnamese_titles().contains_key(&format!("tweak.{}.title", tweak.id)),
                "{} has no Vietnamese title",
                tweak.id
            );
        }
    }

    #[test]
    fn a_cleaning_question_says_permanently_in_both_languages_and_names_the_rule() {
        let ask = ConfirmRequest {
            action: ConfirmAction::CleanRule,
            tweak: "windows-temp".to_owned(),
            title: "Windows temporary files".to_owned(),
            risk: Risk::Caution,
        };
        let english = prompt_text(Lang::English, &ask, Duration::from_secs(45));
        assert!(english.heading.contains("Windows temporary files"));
        assert!(english.heading.contains("permanently"));
        assert!(english.body.contains("cannot be restored"));
        assert_eq!(english.accept, "Delete permanently");
        let vietnamese = prompt_text(Lang::Vietnamese, &ask, Duration::from_secs(45));
        assert!(vietnamese.heading.contains("Tệp tạm của Windows"));
        assert!(vietnamese.heading.contains("vĩnh viễn"));
        assert_eq!(vietnamese.cancel, "Hủy");
    }

    #[test]
    fn every_machine_cleaning_rule_has_a_vietnamese_name() {
        let rules =
            volt_core::CleaningRules::from_toml(include_str!("../../../catalogue/cleaning.toml"))
                .expect("the rules load");
        for rule in rules.rules().iter().filter(|rule| rule.requires_admin) {
            assert!(
                vietnamese_titles().contains_key(&format!("rule.{}.title", rule.id)),
                "{} has no Vietnamese title",
                rule.id
            );
        }
    }

    #[test]
    fn a_restore_point_is_announced_for_a_caution_apply_only() {
        let apply = prompt_text(
            Lang::English,
            &request(ConfirmAction::ApplyTweak, Risk::Caution),
            Duration::from_secs(45),
        );
        assert!(apply.body.contains("restore point"));
        let safe = prompt_text(
            Lang::English,
            &request(ConfirmAction::ApplyTweak, Risk::Safe),
            Duration::from_secs(45),
        );
        assert!(!safe.body.contains("restore point"));
        let revert = prompt_text(
            Lang::English,
            &request(ConfirmAction::RevertTweak, Risk::Caution),
            Duration::from_secs(45),
        );
        assert!(!revert.body.contains("restore point"));
        assert_eq!(revert.accept, "Turn off");
    }
}
