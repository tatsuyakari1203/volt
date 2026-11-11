//! Sentences the core builds from data, as the UI receives them.
//!
//! The core describes such a sentence as a [`volt_core::Phrase`]: a code and named parameters.
//! The window looks the code up in its own message files and writes the sentence in the language
//! of the user; the English sentence that travels next to the phrase is the fallback for a code
//! the window does not know and for records written before the phrase existed (see
//! `volt_core::phrase`).

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use specta::Type;
use volt_core::{Arg, Phrase};

use crate::language::Lang;

/// A value of a phrase. Numbers are `f64` because that is what crosses the boundary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", content = "value", rename_all = "camelCase")]
pub enum ArgView {
    /// Text that is the same in every language (a name, a path, a registry value).
    Text(String),
    /// A whole number.
    Count(f64),
    /// A number in tenths, always shown with one decimal: `35` is 3.5.
    Tenths(f64),
    /// A size of files in bytes, shown like `3.4 MB`.
    Size(f64),
    /// A memory or disk size in bytes, shown like the Windows shell does (`12.4 GB`).
    Bytes(f64),
    /// Another phrase.
    Phrase(PhraseView),
    /// Several phrases one after another, joined by `sep`.
    List {
        /// What goes between two items.
        sep: String,
        /// The items, in order.
        items: Vec<PhraseView>,
    },
}

/// A sentence, or a part of one, as a code and parameters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PhraseView {
    /// Stable code, such as `summary.applied`.
    pub code: String,
    /// The values, by placeholder name.
    pub args: BTreeMap<String, ArgView>,
}

// The numbers come from whole counts and sizes, never from a division, so none is NaN and equality
// is reflexive: the views may be `Eq` like the types that hold them.
impl Eq for ArgView {}
impl Eq for PhraseView {}

/// The Vietnamese templates: the very file the window reads, so the tray and the window write
/// every sentence from one table.
static VIETNAMESE: OnceLock<BTreeMap<String, String>> = OnceLock::new();

fn vietnamese() -> &'static BTreeMap<String, String> {
    VIETNAMESE.get_or_init(|| {
        // A file that does not parse leaves the table empty: every sentence is then English.
        serde_json::from_str(include_str!(
            "../../src/shared/i18n/locales/vi.phrases.json"
        ))
        .unwrap_or_default()
    })
}

/// The sentence of a phrase in `lang`, for the places where the shell writes for the user with no
/// window to do it (the tray menu, the tooltip, the Boost notification). Vietnamese comes from
/// `vi.phrases.json` with a decimal comma; a code that file lacks is written in English.
#[must_use]
pub(crate) fn write(phrase: &Phrase, lang: Lang) -> String {
    match lang {
        Lang::English => phrase.english(),
        Lang::Vietnamese => phrase
            .write_with(&|code| vietnamese().get(code).cloned(), ',', false)
            .unwrap_or_else(|| phrase.english()),
    }
}

/// A count or size of the core as the number the UI gets.
#[allow(
    clippy::cast_precision_loss,
    reason = "counts and sizes above 2^53 are beyond anything a PC holds"
)]
fn number(value: u64) -> f64 {
    value as f64
}

impl From<&Arg> for ArgView {
    fn from(arg: &Arg) -> Self {
        match arg {
            Arg::Text(text) => Self::Text(text.clone()),
            Arg::Count(value) => Self::Count(number(*value)),
            Arg::Tenths(value) => Self::Tenths(number(*value)),
            Arg::Size(value) => Self::Size(number(*value)),
            Arg::Bytes(value) => Self::Bytes(number(*value)),
            Arg::Phrase(phrase) => Self::Phrase(phrase.into()),
            Arg::List { sep, items } => Self::List {
                sep: sep.clone(),
                items: items.iter().map(PhraseView::from).collect(),
            },
        }
    }
}

impl From<&Phrase> for PhraseView {
    fn from(phrase: &Phrase) -> Self {
        Self {
            code: phrase.code.clone(),
            args: phrase
                .args
                .iter()
                .map(|(name, arg)| (name.clone(), ArgView::from(arg)))
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_phrase_keeps_its_code_and_every_parameter() {
        let phrase = Phrase::new("summary.applied")
            .phrase(
                "subject",
                Phrase::new("subject.command").text("program", "x.exe"),
            )
            .size("size", 2048)
            .list(
                "parts",
                ", ",
                vec![Phrase::new("scan.tooNew").count("count", 2_u32)],
            );
        let view = PhraseView::from(&phrase);
        assert_eq!(view.code, "summary.applied");
        assert_eq!(view.args["size"], ArgView::Size(2048.0));
        let ArgView::Phrase(subject) = &view.args["subject"] else {
            panic!("the subject is a phrase");
        };
        assert_eq!(subject.args["program"], ArgView::Text("x.exe".to_owned()));
        let json = serde_json::to_value(&view).expect("serialize");
        assert_eq!(json["args"]["size"]["kind"], "size");
        assert_eq!(json["args"]["parts"]["value"]["sep"], ", ");
    }

    #[test]
    fn the_shell_writes_a_phrase_in_the_chosen_language() {
        let phrase = Phrase::new("boost.notice.moved")
            .phrase("files", Phrase::new("count.file").count("count", 3_u32));
        assert_eq!(
            write(&phrase, Lang::English),
            "3 files moved to the Recycle Bin."
        );
        assert_eq!(
            write(&phrase, Lang::Vietnamese),
            "Đã chuyển 3 tệp vào Thùng rác."
        );
        let memory = Phrase::new("boost.tooltip.memory")
            .bytes("before", 4 << 30)
            .bytes("after", 11 << 29);
        assert_eq!(
            write(&memory, Lang::Vietnamese),
            "Volt: Boost xong, bộ nhớ khả dụng 4,0 GB lên 5,5 GB"
        );
    }

    #[test]
    fn a_code_the_vietnamese_table_lacks_falls_back_to_english() {
        let phrase = Phrase::new("no.such.code");
        assert_eq!(write(&phrase, Lang::Vietnamese), phrase.english());
    }
}
