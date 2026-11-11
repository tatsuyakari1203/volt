//! The failpoint of the elevated CI harness: a helper that ends itself between the journal write
//! and the change.
//!
//! This module exists ONLY in the harness build (the `elevated-ci` feature, which a release build
//! refuses, see `lib.rs`). The product helper holds none of it: no function, no constant, no
//! catalogue entry. The CI step `scripts/check-helper-harness.ps1` searches the release helper for
//! [`MARKER`], the id of the test tweak and the name of its key, and the release workflow runs the
//! same search on the helper it bundles.
//!
//! How it works: the harness build wraps the helper's registry port. A write to the throwaway key of
//! the crash tweak ([`CRASH_KEY`]) ends the process on the spot, without unwinding or closing the
//! journal. The engine journals the previous value BEFORE it changes anything, so what is left is
//! exactly an interrupted apply: a pending journal entry and no change. The next helper start
//! settles it (`reconcile_pending`), and the hosted runner shows that.

use std::sync::Arc;

use volt_core::{
    NamedValue, PortError, Registry, RegistryHive, RegistryKeyRef, RegistryValue, RegistryValueRef,
    RegistryView,
};

/// A string that is in the binary only when the failpoint is.
pub const MARKER: &str = "volt-ci-failpoint";

/// The throwaway key the crash tweak writes. Nothing else touches it.
pub const CRASH_KEY: &str = r"SOFTWARE\VoltTest\Crash";

/// The exit code of a helper that ended at the failpoint.
pub const EXIT_CODE: i32 = 77;

/// A registry that ends the process at the first write to [`CRASH_KEY`] and otherwise passes
/// everything to `inner`.
pub struct FailpointRegistry {
    inner: Arc<dyn Registry>,
}

impl std::fmt::Debug for FailpointRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FailpointRegistry").finish_non_exhaustive()
    }
}

impl FailpointRegistry {
    /// Wraps `inner`.
    #[must_use]
    pub fn new(inner: Arc<dyn Registry>) -> Self {
        Self { inner }
    }
}

/// Whether `target` is under the crash key.
fn is_crash_target(target: &RegistryValueRef) -> bool {
    target.hive == RegistryHive::LocalMachine && target.subkey.eq_ignore_ascii_case(CRASH_KEY)
}

fn end_here(what: &str) -> ! {
    log::warn!("{MARKER}: ending the helper before {what}");
    std::process::exit(EXIT_CODE)
}

impl Registry for FailpointRegistry {
    fn list_values(&self, key: &RegistryKeyRef) -> Result<Vec<NamedValue>, PortError> {
        self.inner.list_values(key)
    }

    fn read_value(&self, target: &RegistryValueRef) -> Result<Option<RegistryValue>, PortError> {
        self.inner.read_value(target)
    }

    fn write_value(
        &self,
        target: &RegistryValueRef,
        data: &RegistryValue,
    ) -> Result<(), PortError> {
        if is_crash_target(target) {
            end_here("the change");
        }
        self.inner.write_value(target, data)
    }

    fn delete_value(&self, target: &RegistryValueRef) -> Result<(), PortError> {
        self.inner.delete_value(target)
    }

    fn list_subkeys(&self, key: &RegistryKeyRef) -> Result<Vec<String>, PortError> {
        self.inner.list_subkeys(key)
    }

    fn create_key(&self, key: &RegistryKeyRef) -> Result<(), PortError> {
        self.inner.create_key(key)
    }

    fn delete_key(&self, key: &RegistryKeyRef) -> Result<(), PortError> {
        self.inner.delete_key(key)
    }

    fn key_exists(
        &self,
        hive: RegistryHive,
        subkey: &str,
        view: RegistryView,
    ) -> Result<bool, PortError> {
        self.inner.key_exists(hive, subkey, view)
    }
}

#[cfg(test)]
mod tests {
    use volt_core::fakes::FakeRegistry;

    use super::*;

    #[test]
    fn only_the_crash_key_is_a_failpoint() {
        let crash = RegistryValueRef::new(RegistryHive::LocalMachine, CRASH_KEY, "Marker");
        let other = RegistryValueRef::new(
            RegistryHive::LocalMachine,
            r"SOFTWARE\VoltTest\Ci",
            "Marker",
        );
        assert!(is_crash_target(&crash));
        assert!(!is_crash_target(&other));
        // Anything else passes through unchanged (the crash write itself cannot be tested: it ends
        // the test process; the hosted runner does that).
        let inner = Arc::new(FakeRegistry::new());
        let registry = FailpointRegistry::new(inner.clone());
        registry
            .write_value(&other, &RegistryValue::Dword(1))
            .expect("write");
        assert_eq!(
            registry.read_value(&other),
            Ok(Some(RegistryValue::Dword(1)))
        );
    }
}
