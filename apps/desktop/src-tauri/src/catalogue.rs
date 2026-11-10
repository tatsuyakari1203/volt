//! The tweak catalogue and the cleaning rules the app ships with.
//!
//! The file lives at `catalogue/tweaks.toml` in the repository root and is embedded at compile
//! time, so the app has no catalogue file to lose, edit or misread at run time. `volt-core` only
//! ever sees the text: [`volt_core::Catalogue::from_toml`] validates it, and the test below
//! proves that the embedded text loads, so an invalid catalogue cannot be released.

/// The embedded catalogue text: the starter tweaks across Performance, Privacy and Appearance.
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
        assert!(
            catalogue.len() >= 30,
            "the starter catalogue has about 30 tweaks"
        );
    }

    #[test]
    fn show_file_extensions_keeps_its_honest_labels() {
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

/// The embedded cleaning rules text (`catalogue/cleaning.toml`), loaded at start exactly like the
/// tweak catalogue: `volt-core` only ever sees the text, [`volt_core::CleaningRules::from_toml`]
/// validates it, and a file that does not validate is a typed
/// [`InitError`](crate::wiring::InitError), never a panic.
pub const EMBEDDED_CLEANING_RULES: &str = include_str!("../../../../catalogue/cleaning.toml");

/// The fingerprints of the two embedded catalogues. The elevated helper is built from the same
/// files and reports its own in `hello`; the UI refuses a helper whose hashes differ.
#[must_use]
pub fn embedded_hashes() -> volt_core::CatalogueHashes {
    volt_core::CatalogueHashes {
        tweaks: volt_core::catalogue_hash(EMBEDDED_CATALOGUE),
        cleaning: volt_core::catalogue_hash(EMBEDDED_CLEANING_RULES),
    }
}

#[cfg(test)]
mod cleaning_tests {
    use volt_core::{CleaningRules, Location, Risk};

    use super::*;

    fn rules() -> CleaningRules {
        CleaningRules::from_toml(EMBEDDED_CLEANING_RULES).expect("the embedded rules are valid")
    }

    #[test]
    fn the_embedded_cleaning_rules_load_and_validate() {
        assert!(rules().len() >= 10, "the core rule set is present");
    }

    #[test]
    fn every_rule_explains_itself() {
        for rule in rules().rules() {
            assert!(
                rule.description.chars().count() >= 200,
                "{}: the description must say what the files are, why removing them is safe and \
                 what the user notices afterwards",
                rule.id
            );
            assert!(!rule.title.trim().is_empty(), "{}", rule.id);
        }
    }

    #[test]
    fn machine_wide_locations_require_administrator_rights_and_per_user_ones_do_not() {
        for rule in rules().rules() {
            let machine_wide = matches!(
                rule.root.location(),
                Location::ProgramData | Location::Windows
            );
            assert_eq!(
                rule.requires_admin, machine_wide,
                "{} ({})",
                rule.id, rule.root
            );
        }
    }

    #[test]
    fn nothing_that_is_a_permanent_delete_or_needs_a_service_restart_is_a_rule() {
        // The Windows Update download cache needs the update service stopped and started again,
        // and the Recycle Bin can only be emptied by a permanent delete: neither is a rule.
        for rule in rules().rules() {
            let root = rule.root.to_string().to_lowercase();
            assert!(!root.contains("softwaredistribution"), "{}", rule.id);
            assert!(!root.contains("recycle"), "{}", rule.id);
        }
    }

    #[test]
    fn nothing_in_the_shipped_set_is_advanced() {
        assert!(
            rules()
                .rules()
                .iter()
                .all(|rule| rule.risk != Risk::Advanced)
        );
    }
}
