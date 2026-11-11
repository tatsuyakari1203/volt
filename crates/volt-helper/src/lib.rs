//! The elevated helper (a composition root over `volt-core` and `volt-win`).
//!
//! `volt-helper.exe` is a second, small program that runs with administrator rights, has no
//! window, no shell, no child processes and no network, and serves ONE connection from the Volt
//! UI that started it. The UI itself never runs elevated. See the decision on the helper for why
//! it is shaped this way; this crate is the helper's half of it:
//!
//! - [`run`]: what `main` does, in order: harden the process, parse the strict command line,
//!   check the install folder, verify the UI it is bound to, create the pipe, serve;
//! - [`serve`]: the serve loop with its clocks (idle limit, hard cap), the bound-client check
//!   and the exit when the client goes away;
//! - [`session`]: one connection's protocol: ordering, quotas and the requests, over the core's
//!   wire types and the core's memory operations;
//! - [`state`]: the helper's own protected state folder, the single-helper lock and its store;
//! - [`tweaks`]: administrator tweaks over the helper's own journal, behind [`confirm`]'s
//!   confirmation (drawn on a private desktop, see [`confirm`] and [`prompt_text`]) and, for a
//!   Caution or Advanced tweak, a restore point first, through the guarded [`ports`].
//!
//! The logic lives here, in the library, so tests run it in-process without elevation. The binary
//! adds only `main`, an elevation manifest in release builds and, outside release builds only, two
//! test modes (compiled out of release builds by `cfg`, never switched by a flag).

#![cfg(windows)]

// The accepting confirmer of the CI harness must never reach a helper that asks for administrator
// rights: the release profile (which the build script turns into `helper_release`) refuses the
// harness feature outright.
#[cfg(all(helper_release, feature = "elevated-ci"))]
compile_error!("the `elevated-ci` feature must never be enabled in a release build of the helper");

pub mod confirm;
#[cfg(not(helper_release))]
pub mod debug;
pub mod ports;
pub mod prompt_text;
pub mod run;
pub mod serve;
pub mod session;
pub mod state;
pub mod tweaks;

use std::sync::Arc;

use volt_core::{Catalogue, CatalogueHashes, Memory, SystemClock, catalogue_hash};
use volt_win::WinMemory;
use volt_win::helper::restore::WinRestorePoints;

use crate::session::{QuotaLimits, Session};
use crate::state::HelperState;
use crate::tweaks::{DeskParts, TweakDesk};

/// The tweak catalogue of the product.
const PRODUCT_TWEAK_CATALOGUE: &str = include_str!("../../../catalogue/tweaks.toml");
/// The tweak catalogue this helper holds. The UI compares its hash with its own.
#[cfg(not(feature = "elevated-ci"))]
const TWEAK_CATALOGUE: &str = PRODUCT_TWEAK_CATALOGUE;
/// The catalogue of the harness build: the product's, plus one test-only tweak under a throwaway
/// `HKLM\SOFTWARE\VoltTest` key. The release helper never has it.
#[cfg(feature = "elevated-ci")]
const TWEAK_CATALOGUE: &str = concat!(
    include_str!("../../../catalogue/tweaks.toml"),
    "\n",
    include_str!("../ci/ci-tweak.toml")
);
/// The cleaning rules this helper holds.
const CLEANING_RULES: &str = include_str!("../../../catalogue/cleaning.toml");

/// The text of the tweak catalogue this helper holds (the harness of the elevated CI job reads
/// the tweaks it exercises from it).
#[must_use]
pub fn embedded_tweak_catalogue() -> &'static str {
    TWEAK_CATALOGUE
}

/// The hashes of the catalogues of the PRODUCT: what the app embeds and what the release helper
/// holds. They equal [`catalogue_hashes`] in every build except the harness build of the CI job,
/// whose catalogue has a test-only tweak more.
#[must_use]
pub fn product_catalogue_hashes() -> CatalogueHashes {
    CatalogueHashes {
        tweaks: catalogue_hash(PRODUCT_TWEAK_CATALOGUE),
        cleaning: catalogue_hash(CLEANING_RULES),
    }
}

/// The hashes of the catalogues this helper was built with.
#[must_use]
pub fn catalogue_hashes() -> CatalogueHashes {
    CatalogueHashes {
        tweaks: catalogue_hash(TWEAK_CATALOGUE),
        cleaning: catalogue_hash(CLEANING_RULES),
    }
}

/// The session the real helper serves: memory purges on the real memory port, the real
/// catalogues, the decision's quotas, and the administrator tweaks over the helper's own `state`
/// behind [`confirm::product_confirmer`].
///
/// A start that cannot build the tweak desk (an interrupted apply whose journal cannot be read, a
/// catalogue that breaks the administrator rules) still serves memory purges and answers every
/// tweak request [`volt_core::ErrorCode::Unavailable`]: it never serves tweaks it could not
/// settle.
#[must_use]
pub fn production_session(state: &HelperState) -> Session {
    let memory: Arc<dyn Memory> = Arc::new(WinMemory::new());
    let session = Session::new(
        Arc::clone(&memory),
        catalogue_hashes(),
        env!("CARGO_PKG_VERSION"),
        QuotaLimits::default(),
    );
    match build_desk(state, memory) {
        Ok(desk) => session.with_tweaks(desk),
        Err(error) => {
            log::error!("administrator tweaks are not available in this helper: {error}");
            session
        }
    }
}

fn build_desk(state: &HelperState, memory: Arc<dyn Memory>) -> Result<TweakDesk, String> {
    let catalogue = Arc::new(Catalogue::from_toml(TWEAK_CATALOGUE).map_err(|e| e.to_string())?);
    let store = state.store();
    TweakDesk::new(DeskParts {
        ports: ports::helper_ports(&catalogue, memory),
        catalogue,
        journal: store.clone(),
        activity: store,
        clock: Arc::new(SystemClock),
        confirmer: confirm::product_confirmer(),
        restore: Arc::new(WinRestorePoints::new()),
        store_id: state.store_id().to_owned(),
    })
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_catalogue_is_fit_for_the_helper() {
        // The build check: a tweak that requires administrator rights must have neither a command
        // step nor a per-user registry step.
        let catalogue = Catalogue::from_toml(TWEAK_CATALOGUE).expect("the catalogue loads");
        assert_eq!(catalogue.administrator_issues(), Vec::new());
        assert!(
            catalogue.tweaks().iter().any(|tweak| tweak.requires_admin),
            "the helper has administrator tweaks to offer"
        );
    }

    #[cfg(not(feature = "elevated-ci"))]
    #[test]
    fn a_product_catalogue_has_no_test_tweak() {
        assert!(!TWEAK_CATALOGUE.contains("VoltTest"));
    }

    #[test]
    fn the_embedded_catalogues_are_hashed() {
        let hashes = catalogue_hashes();
        assert_eq!(hashes.tweaks.len(), 64);
        assert_eq!(hashes.cleaning.len(), 64);
        assert_ne!(hashes.tweaks, hashes.cleaning);
        assert_eq!(hashes, catalogue_hashes());
    }
}
