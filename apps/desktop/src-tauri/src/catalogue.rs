//! The tweak catalogue the app ships with.
//!
//! The file lives at `catalogue/tweaks.toml` in the repository root and is embedded at compile
//! time, so the app has no catalogue file to lose, edit or misread at run time. `volt-core` only
//! ever sees the text: [`volt_core::Catalogue::from_toml`] validates it, and the test below
//! proves that the embedded text loads, so an invalid catalogue cannot be released.

/// The embedded catalogue text. Today it holds one tweak; the real catalogue is a later task.
pub const EMBEDDED_CATALOGUE: &str = include_str!("../../../../catalogue/tweaks.toml");

#[cfg(test)]
mod tests {
    use volt_core::{
        Catalogue, Category, Evidence, RegistryHive, RegistryValue, RestartRequirement, Risk,
        TweakStep,
    };

    use super::*;

    #[test]
    fn the_embedded_catalogue_loads_and_validates() {
        let catalogue =
            Catalogue::from_toml(EMBEDDED_CATALOGUE).expect("the embedded catalogue is valid");
        assert_eq!(catalogue.len(), 1, "the foundation ships exactly one tweak");
    }

    #[test]
    fn the_one_tweak_is_show_file_extensions_with_honest_labels() {
        let catalogue = Catalogue::from_toml(EMBEDDED_CATALOGUE).expect("valid");
        let tweak = catalogue
            .get("show-file-extensions")
            .expect("the tweak exists");
        assert_eq!(tweak.title, "Show file extensions");
        assert_eq!(tweak.category, Category::Appearance);
        assert_eq!(tweak.risk, Risk::Safe);
        // A preference, not a speed-up: it must not claim a measured benefit.
        assert_eq!(tweak.evidence, Evidence::Anecdotal);
        assert!(tweak.evidence_note.contains("not a performance gain"));
        assert_eq!(tweak.restart, RestartRequirement::None);
        assert_eq!(tweak.steps.len(), 1);
        assert!(matches!(
            &tweak.steps[0],
            TweakStep::RegistrySet { hive, subkey, name, value, .. }
                if *hive == RegistryHive::CurrentUser
                    && subkey == r"Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced"
                    && name == "HideFileExt"
                    && *value == RegistryValue::Dword(0)
        ));
    }
}
