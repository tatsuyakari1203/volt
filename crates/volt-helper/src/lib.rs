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
//!   wire types and the core's memory operations.
//!
//! The logic lives here, in the library, so tests run it in-process without elevation. The binary
//! adds only `main`, an elevation manifest in release builds and, outside release builds only, two
//! test modes (compiled out of release builds by `cfg`, never switched by a flag).

#![cfg(windows)]

#[cfg(not(helper_release))]
pub mod debug;
pub mod run;
pub mod serve;
pub mod session;

use std::sync::Arc;

use volt_core::{CatalogueHashes, Memory, catalogue_hash};
use volt_win::WinMemory;

use crate::session::{QuotaLimits, Session};

/// The tweak catalogue this helper holds. The UI compares its hash with its own.
const TWEAK_CATALOGUE: &str = include_str!("../../../catalogue/tweaks.toml");
/// The cleaning rules this helper holds.
const CLEANING_RULES: &str = include_str!("../../../catalogue/cleaning.toml");

/// The hashes of the catalogues this helper was built with.
#[must_use]
pub fn catalogue_hashes() -> CatalogueHashes {
    CatalogueHashes {
        tweaks: catalogue_hash(TWEAK_CATALOGUE),
        cleaning: catalogue_hash(CLEANING_RULES),
    }
}

/// The session the real helper serves: memory purges on the real memory port, the real
/// catalogues, the decision's quotas.
#[must_use]
pub fn production_session() -> Session {
    let memory: Arc<dyn Memory> = Arc::new(WinMemory::new());
    Session::new(
        memory,
        catalogue_hashes(),
        env!("CARGO_PKG_VERSION"),
        QuotaLimits::default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_catalogues_are_hashed() {
        let hashes = catalogue_hashes();
        assert_eq!(hashes.tweaks.len(), 64);
        assert_eq!(hashes.cleaning.len(), 64);
        assert_ne!(hashes.tweaks, hashes.cleaning);
        assert_eq!(hashes, catalogue_hashes());
    }
}
