//! The `StartupApproved` records Windows keeps for enabled and disabled startup
//! entries.
//!
//! Task Manager and the Settings app do not delete a Run value or a shortcut to
//! switch an entry off. They write a small binary value of the same name under
//! `...\Explorer\StartupApproved\<kind>`, and Windows reads it at sign-in. Volt
//! uses that same mechanism, so every tool shows the same state.
//!
//! # Layout (`REG_BINARY`, 12 bytes)
//!
//! | Bytes | Meaning                                                                   |
//! | ----- | ------------------------------------------------------------------------- |
//! | 0..4  | Flags, a little-endian `u32`: `0x02` or `0x06` enabled, `0x03` disabled    |
//! | 4..12 | A little-endian `FILETIME`: when it was disabled; all zeros when enabled   |
//!
//! Source: the layout is described by practitioner write-ups of the registry
//! artefact (Harlan Carvey, "Does autostart really mean autostart?", windowsir
//! blog, 2022-07-09, and "StartupApproved\Run, pt II", 2022-07-17); Microsoft
//! does not document it. It was also checked by reading this development
//! machine's keys read-only: every value was 12 bytes, and the first `u32` was
//! `0x03` with a non-zero `FILETIME` for entries switched off in Task Manager,
//! `0x02` or `0x06` with zeros for enabled ones. One value began with `0x01`,
//! which no source explains: Volt treats a record it does not recognise as
//! "enabled" for display and never invents a meaning for it.
//!
//! One sub-key per source: `Run` (the Run keys of the 64-bit view, per user and
//! machine-wide), `Run32` (the 32-bit machine-wide Run key under
//! `WOW6432Node`) and `StartupFolder` (the per-user and the common Startup
//! folders). The value name is the Run value name, or the file name with its
//! extension for a Startup folder item. Values written under `HKCU` apply to
//! the user, values under `HKLM` to every user.
//!
//! Disabling writes `03 00 00 00` plus the current `FILETIME`. Enabling writes
//! `02 00 00 00` plus eight zero bytes, which is what Task Manager writes when
//! an entry is switched back on (the value is kept, not deleted).

use crate::registry::{RegistryValue, RegistryValueRef};
use crate::time::Timestamp;

/// The registry key name that holds the approval records.
const APPROVED_KEY: &str = "StartupApproved";
/// The sub-keys of [`APPROVED_KEY`] that hold records, one per source kind.
const APPROVED_KINDS: [&str; 3] = ["Run", "Run32", "StartupFolder"];

/// Flags of an enabled record (Task Manager writes this when it switches an
/// entry on). `0x06` is also seen in the wild and means enabled.
const FLAGS_ENABLED: u32 = 0x02;
const FLAGS_ENABLED_ALTERNATE: u32 = 0x06;
/// Flags of a disabled record.
const FLAGS_DISABLED: u32 = 0x03;

/// Milliseconds between 1601-01-01 (the `FILETIME` epoch) and 1970-01-01.
const FILETIME_EPOCH_OFFSET_MS: i64 = 11_644_473_600_000;
/// `FILETIME` ticks (100 ns) per millisecond.
const TICKS_PER_MS: i64 = 10_000;

/// Whether a startup entry runs at sign-in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StartupState {
    /// The entry starts with Windows.
    Enabled,
    /// The entry is switched off; it stays where it is but does not start.
    Disabled,
}

/// Reads an approval record. `None` when the value is not a record this layout
/// describes (not binary, shorter than four bytes, or flags nobody documents).
#[must_use]
pub fn decode_approval(value: &RegistryValue) -> Option<StartupState> {
    let RegistryValue::Binary(bytes) = value else {
        return None;
    };
    let flags = u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?);
    match flags {
        FLAGS_DISABLED => Some(StartupState::Disabled),
        FLAGS_ENABLED | FLAGS_ENABLED_ALTERNATE => Some(StartupState::Enabled),
        _ => None,
    }
}

/// Builds the record to write: disabled carries `now` as a `FILETIME`, enabled
/// carries zeros.
#[must_use]
pub fn encode_approval(state: StartupState, now: Timestamp) -> RegistryValue {
    let (flags, filetime) = match state {
        StartupState::Enabled => (FLAGS_ENABLED, 0_u64),
        StartupState::Disabled => (FLAGS_DISABLED, filetime_of(now)),
    };
    let mut bytes = Vec::with_capacity(12);
    bytes.extend_from_slice(&flags.to_le_bytes());
    bytes.extend_from_slice(&filetime.to_le_bytes());
    RegistryValue::Binary(bytes)
}

/// `now` as a `FILETIME`; zero for a time before 1601.
fn filetime_of(now: Timestamp) -> u64 {
    let ticks = now
        .as_unix_millis()
        .saturating_add(FILETIME_EPOCH_OFFSET_MS)
        .saturating_mul(TICKS_PER_MS);
    u64::try_from(ticks).unwrap_or(0)
}

/// The entry name when `target` is an approval record, that is a value in a key
/// whose last two components are `StartupApproved` and one of `Run`, `Run32`
/// or `StartupFolder` (compared ignoring case). This is how the activity log
/// and the Undo page recognise the change in a journal entry, which holds a
/// plain registry operation.
#[must_use]
pub fn approved_entry_name(target: &RegistryValueRef) -> Option<&str> {
    if target.name.is_empty() {
        return None;
    }
    let mut parts = target.subkey.rsplit('\\');
    let kind = parts.next()?;
    let parent = parts.next()?;
    (parent.eq_ignore_ascii_case(APPROVED_KEY)
        && APPROVED_KINDS
            .iter()
            .any(|known| known.eq_ignore_ascii_case(kind)))
    .then_some(target.name.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::RegistryHive;

    fn binary(bytes: &[u8]) -> RegistryValue {
        RegistryValue::Binary(bytes.to_vec())
    }

    #[test]
    fn the_flags_decide_the_state() {
        let record = |flags: u8| {
            let mut bytes = vec![flags, 0, 0, 0];
            bytes.extend_from_slice(&[0; 8]);
            decode_approval(&binary(&bytes))
        };
        assert_eq!(record(0x02), Some(StartupState::Enabled));
        assert_eq!(record(0x06), Some(StartupState::Enabled));
        assert_eq!(record(0x03), Some(StartupState::Disabled));
        // Unknown flags are not guessed at.
        assert_eq!(record(0x01), None);
        assert_eq!(record(0x00), None);
        assert_eq!(record(0x07), None);
    }

    #[test]
    fn a_record_that_is_not_a_record_decodes_to_nothing() {
        assert_eq!(decode_approval(&binary(&[])), None);
        assert_eq!(decode_approval(&binary(&[3, 0, 0])), None);
        assert_eq!(decode_approval(&RegistryValue::Dword(3)), None);
        assert_eq!(decode_approval(&RegistryValue::Sz("x".into())), None);
        // The flags are a whole u32: a stray high byte is not a record.
        assert_eq!(decode_approval(&binary(&[3, 0, 0, 1])), None);
        // Extra trailing bytes do not matter.
        assert_eq!(
            decode_approval(&binary(&[3, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9])),
            Some(StartupState::Disabled)
        );
    }

    #[test]
    fn enabled_records_are_flags_two_and_zeros() {
        assert_eq!(
            encode_approval(StartupState::Enabled, Timestamp::from_unix_millis(5)),
            binary(&[2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0])
        );
    }

    #[test]
    fn disabled_records_carry_the_time_as_a_filetime() {
        // 2023-11-14T22:13:20.123Z, a time whose FILETIME is known:
        // (1_700_000_000_123 + 11_644_473_600_000) * 10_000 = 133_444_736_001_230_000.
        let value = encode_approval(
            StartupState::Disabled,
            Timestamp::from_unix_millis(1_700_000_000_123),
        );
        let RegistryValue::Binary(bytes) = &value else {
            panic!("binary expected");
        };
        assert_eq!(bytes.len(), 12);
        assert_eq!(bytes[..4], [3, 0, 0, 0]);
        assert_eq!(
            u64::from_le_bytes(bytes[4..].try_into().expect("8 bytes")),
            133_444_736_001_230_000
        );
        assert_eq!(decode_approval(&value), Some(StartupState::Disabled));
    }

    #[test]
    fn the_unix_epoch_is_the_known_filetime_offset() {
        let RegistryValue::Binary(bytes) =
            encode_approval(StartupState::Disabled, Timestamp::from_unix_millis(0))
        else {
            panic!("binary expected");
        };
        assert_eq!(
            u64::from_le_bytes(bytes[4..].try_into().expect("8 bytes")),
            116_444_736_000_000_000
        );
    }

    #[test]
    fn a_time_before_1601_encodes_as_zero_instead_of_wrapping() {
        let RegistryValue::Binary(bytes) = encode_approval(
            StartupState::Disabled,
            Timestamp::from_unix_millis(-20_000_000_000_000),
        ) else {
            panic!("binary expected");
        };
        assert_eq!(bytes[4..], [0; 8]);
    }

    #[test]
    fn approval_targets_are_recognised_by_their_key() {
        let target = |subkey: &str, name: &str| {
            RegistryValueRef::new(RegistryHive::CurrentUser, subkey, name)
        };
        let key = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved";
        for kind in ["Run", "Run32", "StartupFolder", "run", "STARTUPFOLDER"] {
            let subkey = format!(r"{key}\{kind}");
            assert_eq!(
                approved_entry_name(&target(&subkey, "Sample App")),
                Some("Sample App"),
                "{kind}"
            );
        }
        // A test sandbox that mirrors the layout is recognised too.
        assert_eq!(
            approved_entry_name(&target(r"Software\VoltTest\x\StartupApproved\Run", "N")),
            Some("N")
        );
        // Not approval records.
        assert_eq!(approved_entry_name(&target(key, "Sample")), None);
        assert_eq!(
            approved_entry_name(&target(r"Software\X\Run", "Sample")),
            None
        );
        assert_eq!(
            approved_entry_name(&target(r"Software\StartupApproved\Other", "Sample")),
            None
        );
        assert_eq!(
            approved_entry_name(&target(r"StartupApproved\Run\deeper", "Sample")),
            None
        );
        assert_eq!(
            approved_entry_name(&target(r"A\StartupApproved\Run", "")),
            None
        );
    }
}
