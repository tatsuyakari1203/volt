//! Reading the installed desktop apps from the Uninstall registry keys.
//!
//! Each app is a subkey of `...\CurrentVersion\Uninstall` in one of three places: the machine's
//! 64-bit view, the machine's 32-bit view and the current user. Only the registry port is used,
//! so the reading is tested with the in-memory registry and with a throwaway key.

use std::collections::HashMap;

use super::command::UninstallCommand;
use super::{AppDate, AppSource, HiddenReason, InstalledApp, OwnInstall, Removal};
use crate::error::PortError;
use crate::registry::{
    NamedValue, Registry, RegistryHive, RegistryKeyRef, RegistryValue, RegistryView,
};

/// The key every location holds the apps under, below its root.
pub(super) const UNINSTALL_SUBKEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall";

/// One place the apps are listed: a registry key in a view, and what it is called.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppLocation {
    /// Root key.
    pub hive: RegistryHive,
    /// 32-bit or 64-bit view.
    pub view: RegistryView,
    /// The key whose subkeys are the apps.
    pub subkey: String,
    /// Which of the three places it is.
    pub source: AppSource,
}

impl AppLocation {
    /// The three places Windows lists apps: the machine in the 64-bit view, the machine in the
    /// 32-bit view and the current user.
    #[must_use]
    pub fn windows() -> Vec<Self> {
        let at = |hive, view, source| Self {
            hive,
            view,
            subkey: UNINSTALL_SUBKEY.to_owned(),
            source,
        };
        vec![
            at(
                RegistryHive::LocalMachine,
                RegistryView::Bit64,
                AppSource::Machine64,
            ),
            at(
                RegistryHive::LocalMachine,
                RegistryView::Bit32,
                AppSource::Machine32,
            ),
            at(
                RegistryHive::CurrentUser,
                RegistryView::Native,
                AppSource::CurrentUser,
            ),
        ]
    }

    /// The same three places moved under `root` in the current user's hive, for tests that must
    /// not read or touch the real machine: `<root>\machine64`, `<root>\machine32` and
    /// `<root>\user`, each holding the apps as subkeys.
    #[must_use]
    pub fn sandboxed_in_hkcu(root: &str) -> Vec<Self> {
        let at = |name: &str, source| Self {
            hive: RegistryHive::CurrentUser,
            view: RegistryView::Native,
            subkey: format!(r"{root}\{name}"),
            source,
        };
        vec![
            at("machine64", AppSource::Machine64),
            at("machine32", AppSource::Machine32),
            at("user", AppSource::CurrentUser),
        ]
    }

    fn key(&self) -> RegistryKeyRef {
        RegistryKeyRef::new(self.hive, self.subkey.clone(), self.view)
    }
}

/// What reading one location gave.
pub(super) struct Read {
    pub(super) apps: Vec<InstalledApp>,
    /// Entries that could not be read and were left out.
    pub(super) skipped: u32,
}

/// Reads every app of `location`. The location itself failing is an error; one entry failing is
/// counted in `skipped`.
pub(super) fn read_location(
    registry: &dyn Registry,
    location: &AppLocation,
    own: &OwnInstall,
) -> Result<Read, PortError> {
    let key = location.key();
    let mut apps = Vec::new();
    let mut skipped = 0;
    for name in registry.list_subkeys(&key)? {
        let entry = RegistryKeyRef::new(key.hive, format!(r"{}\{name}", key.subkey), key.view);
        match registry.list_values(&entry) {
            Ok(values) => apps.extend(app_of(location, &name, &values, own)),
            Err(_) => skipped += 1,
        }
    }
    Ok(Read { apps, skipped })
}

/// The values of one entry by lower-case name, leaving out the ones that could not be read.
struct Values(HashMap<String, RegistryValue>);

impl Values {
    fn of(values: &[NamedValue]) -> Self {
        Self(
            values
                .iter()
                .filter_map(|named| {
                    named
                        .value
                        .as_ref()
                        .ok()
                        .map(|value| (named.name.to_lowercase(), value.clone()))
                })
                .collect(),
        )
    }

    fn text(&self, name: &str) -> Option<String> {
        match self.0.get(&name.to_lowercase())? {
            RegistryValue::Sz(text) | RegistryValue::ExpandSz(text) => {
                let text = text.trim();
                (!text.is_empty()).then(|| text.to_owned())
            }
            _ => None,
        }
    }

    fn number(&self, name: &str) -> Option<u32> {
        match self.0.get(&name.to_lowercase())? {
            RegistryValue::Dword(value) => Some(*value),
            RegistryValue::Sz(text) => text.trim().parse().ok(),
            _ => None,
        }
    }

    fn flag(&self, name: &str) -> bool {
        self.number(name) == Some(1)
    }
}

/// `ReleaseType` values Windows Installer and installers write for updates and patches.
const UPDATE_RELEASE_TYPES: [&str; 5] = [
    "update",
    "hotfix",
    "security update",
    "update rollup",
    "service pack",
];

fn is_update(key_name: &str, values: &Values) -> bool {
    if values.text("ParentKeyName").is_some() || values.text("ParentDisplayName").is_some() {
        return true;
    }
    if let Some(release) = values.text("ReleaseType")
        && UPDATE_RELEASE_TYPES.contains(&release.to_lowercase().as_str())
    {
        return true;
    }
    // Windows updates: `KB1234567`, with or without a suffix.
    let upper = key_name.to_ascii_uppercase();
    upper
        .strip_prefix("KB")
        .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
}

fn install_date(values: &Values) -> Option<AppDate> {
    let text = match values.0.get("installdate")? {
        RegistryValue::Dword(number) => number.to_string(),
        RegistryValue::Sz(text) | RegistryValue::ExpandSz(text) => text.trim().to_owned(),
        _ => return None,
    };
    AppDate::parse_compact(&text)
}

/// One app from the values of its key, or `None` when it has no display name or is Volt itself.
fn app_of(
    location: &AppLocation,
    key_name: &str,
    values: &[NamedValue],
    own: &OwnInstall,
) -> Option<InstalledApp> {
    let values = Values::of(values);
    let name = values.text("DisplayName")?;
    let command = values
        .text("UninstallString")
        .map(|text| UninstallCommand::parse(&text));
    let folders = [
        values.text("InstallLocation"),
        command
            .as_ref()
            .and_then(|parsed| parsed.as_ref().ok())
            .and_then(|parsed| parsed.program_folder().map(str::to_owned)),
    ];
    if own.is_volt(&name, folders.iter().flatten().map(String::as_str)) {
        return None;
    }
    let hidden = if values.flag("SystemComponent") {
        Some(HiddenReason::SystemComponent)
    } else if is_update(key_name, &values) {
        Some(HiddenReason::Update)
    } else if values.flag("NoRemove") {
        Some(HiddenReason::NotRemovable)
    } else if !matches!(command, Some(Ok(_))) {
        Some(HiddenReason::NoUninstallCommand)
    } else {
        None
    };
    let removal = match command {
        Some(Ok(command)) => Removal::Command(command),
        _ => Removal::None,
    };
    Some(InstalledApp {
        id: format!("win32:{}:{key_name}", location.source.id_part()),
        source: location.source,
        name,
        publisher: values.text("Publisher"),
        version: values.text("DisplayVersion"),
        size_bytes: values
            .number("EstimatedSize")
            .filter(|kilobytes| *kilobytes > 0)
            .map(|kilobytes| u64::from(kilobytes) * 1024),
        installed: install_date(&values),
        hidden,
        removal,
    })
}
