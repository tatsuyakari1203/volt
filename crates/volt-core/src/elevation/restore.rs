//! Restore points, as a port.
//!
//! Before the helper applies an administrator tweak whose risk is above Safe it asks Windows to
//! create a system restore point, so the user has a way back that does not depend on Volt. The rules
//! live here; the call that creates the point is the adapter's.
//!
//! - Which tweaks need one: [`needs_restore_point`] (Caution and Advanced).
//! - At most one point per [`RESTORE_POINT_MIN_GAP`] in a session ([`RestoreGate`]): a second
//!   Caution tweak a few minutes later does not make a second point, and the decision's quota of one
//!   per ten minutes holds without a refusal.
//! - A point that cannot be created refuses the tweak ([`crate::ErrorCode::RestorePointFailed`]):
//!   the requirement is the helper's, not the client's, so a failure is never skipped silently.
//!
//! Windows applies its own limit on top (by default one point per 24 hours, with no error when it
//! skips one), which a program cannot see, so a successful call means "Windows accepted the
//! request".

use std::time::{Duration, Instant};

use crate::error::PortError;
use crate::label::Risk;
use crate::registry::{Registry, RegistryHive, RegistryValue, RegistryValueRef};

/// The shortest time between two restore points of one helper session.
pub const RESTORE_POINT_MIN_GAP: Duration = Duration::from_secs(10 * 60);

/// Port: asks Windows to create a system restore point.
pub trait RestorePoints: Send + Sync {
    /// Creates a restore point described by `description` (shown in System Restore).
    ///
    /// # Errors
    /// [`PortError::Unsupported`] when System Protection is off or the service is disabled, another
    /// [`PortError`] when Windows refused or the call failed. Nothing was created.
    fn create(&self, description: &str) -> Result<(), PortError>;
}

/// Whether a tweak of this risk needs a restore point before it is applied.
#[must_use]
pub const fn needs_restore_point(risk: Risk) -> bool {
    !matches!(risk, Risk::Safe)
}

/// Whether System Protection looks turned on, as far as a read without administrator rights can
/// tell.
///
/// It is a hint, never the authority: only a call that tries to make the restore point knows, and
/// the helper refuses a Caution or Advanced tweak whenever that call fails. The UI uses the hint to
/// say what to do (turn on System Protection for the system drive) before the user tries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SystemProtection {
    /// System Protection looks on: restore points can be made.
    On,
    /// System Protection looks off (turned off, or by policy): no restore point can be made.
    Off,
    /// The registry does not say.
    Unknown,
}

/// The key where the System Restore service keeps its state, and the policy key above it.
const SYSTEM_RESTORE_KEY: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\SystemRestore";
const SYSTEM_RESTORE_POLICY_KEY: &str = r"SOFTWARE\Policies\Microsoft\Windows NT\SystemRestore";

/// Reads whether System Protection looks on. Reads two machine registry values (readable by any
/// user) and changes nothing: the policy `DisableSR` set to 1 is off; otherwise the service's own
/// `RPSessionInterval` is 0 when System Protection is off for every drive and 1 when it is on.
/// Anything else, or a value that cannot be read, is [`SystemProtection::Unknown`].
#[must_use]
pub fn system_protection_of(registry: &dyn Registry) -> SystemProtection {
    let read = |key: &str, name: &str| {
        registry
            .read_value(&RegistryValueRef::new(
                RegistryHive::LocalMachine,
                key,
                name,
            ))
            .ok()
            .flatten()
    };
    if read(SYSTEM_RESTORE_POLICY_KEY, "DisableSR") == Some(RegistryValue::Dword(1)) {
        return SystemProtection::Off;
    }
    match read(SYSTEM_RESTORE_KEY, "RPSessionInterval") {
        Some(RegistryValue::Dword(0)) => SystemProtection::Off,
        Some(RegistryValue::Dword(1)) => SystemProtection::On,
        _ => SystemProtection::Unknown,
    }
}

/// Remembers when the last restore point of the session was made.
#[derive(Debug, Default)]
pub struct RestoreGate {
    last: Option<Instant>,
}

impl RestoreGate {
    /// A gate that has seen no restore point.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Makes sure a restore point exists for a change at `now`: creates one through `points`
    /// unless one was made less than [`RESTORE_POINT_MIN_GAP`] ago. Returns whether a new one was
    /// created.
    ///
    /// # Errors
    /// The port's error; nothing was created and the caller must not change anything.
    pub fn ensure(
        &mut self,
        points: &dyn RestorePoints,
        description: &str,
        now: Instant,
    ) -> Result<bool, PortError> {
        if self
            .last
            .is_some_and(|at| now.saturating_duration_since(at) < RESTORE_POINT_MIN_GAP)
        {
            return Ok(false);
        }
        points.create(description)?;
        self.last = Some(now);
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::elevation::fakes::FakeRestorePoints;

    #[test]
    fn only_a_tweak_above_safe_needs_a_restore_point() {
        assert!(!needs_restore_point(Risk::Safe));
        assert!(needs_restore_point(Risk::Caution));
        assert!(needs_restore_point(Risk::Advanced));
    }

    #[test]
    fn system_protection_is_read_from_the_service_state_and_the_policy() {
        use crate::fakes::FakeRegistry;
        let key =
            |name: &str, key: &str| RegistryValueRef::new(RegistryHive::LocalMachine, key, name);
        let registry = FakeRegistry::new();
        assert_eq!(system_protection_of(&registry), SystemProtection::Unknown);
        registry.set(
            &key("RPSessionInterval", SYSTEM_RESTORE_KEY),
            RegistryValue::Dword(1),
        );
        assert_eq!(system_protection_of(&registry), SystemProtection::On);
        registry.set(
            &key("RPSessionInterval", SYSTEM_RESTORE_KEY),
            RegistryValue::Dword(0),
        );
        assert_eq!(system_protection_of(&registry), SystemProtection::Off);
        registry.set(
            &key("RPSessionInterval", SYSTEM_RESTORE_KEY),
            RegistryValue::Dword(1),
        );
        // The policy wins over the service's state.
        registry.set(
            &key("DisableSR", SYSTEM_RESTORE_POLICY_KEY),
            RegistryValue::Dword(1),
        );
        assert_eq!(system_protection_of(&registry), SystemProtection::Off);
        registry.set(
            &key("DisableSR", SYSTEM_RESTORE_POLICY_KEY),
            RegistryValue::Dword(0),
        );
        assert_eq!(system_protection_of(&registry), SystemProtection::On);
    }

    #[test]
    fn one_point_per_ten_minutes_and_a_failure_is_not_remembered() {
        let points = FakeRestorePoints::new();
        let mut gate = RestoreGate::new();
        let start = Instant::now();
        assert_eq!(gate.ensure(&points, "a", start), Ok(true));
        assert_eq!(
            gate.ensure(&points, "b", start + Duration::from_secs(599)),
            Ok(false)
        );
        assert_eq!(
            gate.ensure(&points, "c", start + RESTORE_POINT_MIN_GAP),
            Ok(true)
        );
        assert_eq!(points.descriptions(), vec!["a".to_owned(), "c".to_owned()]);

        let failing = FakeRestorePoints::failing();
        let mut gate = RestoreGate::new();
        assert!(gate.ensure(&failing, "x", start).is_err());
        // The failure left no memory of a point: the next change tries again.
        assert!(gate.ensure(&failing, "x", start).is_err());
        assert_eq!(failing.attempts(), 2);
    }
}
