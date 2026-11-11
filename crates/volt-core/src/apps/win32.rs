//! Reading the installed desktop apps from the Uninstall registry keys.
//!
//! Each app is a subkey of `...\CurrentVersion\Uninstall` in one of three places: the machine's
//! 64-bit view, the machine's 32-bit view and the current user. Only the registry port is used,
//! so the reading is tested with the in-memory registry and with a throwaway key.

use std::collections::HashMap;

use super::command::UninstallCommand;
use super::leftovers::{FolderOrigin, Footprint, RecordedFolder};
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
    let footprint = footprint_of(&name, &values, command.as_ref());
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
        footprint: Some(footprint),
    })
}

/// What the entry says about where the app lives, kept for the leftover scan after an uninstall.
fn footprint_of(
    name: &str,
    values: &Values,
    command: Option<&Result<UninstallCommand, super::CommandRefusal>>,
) -> Footprint {
    let mut folders = Vec::new();
    let mut note = |raw: Option<String>, origin| {
        if let Some(raw) = raw {
            folders.push(RecordedFolder { raw, origin });
        }
    };
    note(
        values.text("InstallLocation"),
        FolderOrigin::InstallLocation,
    );
    note(
        command
            .and_then(|parsed| parsed.as_ref().ok())
            .and_then(|parsed| parsed.program_folder().map(str::to_owned)),
        FolderOrigin::Uninstaller,
    );
    note(
        values
            .text("DisplayIcon")
            .and_then(|icon| icon_folder(&icon)),
        FolderOrigin::Icon,
    );
    let mut keys: Vec<RegistryKeyRef> = values
        .0
        .values()
        .filter_map(|value| match value {
            RegistryValue::Sz(text) | RegistryValue::ExpandSz(text) => registry_path(text),
            _ => None,
        })
        .collect();
    keys.sort_by_key(ToString::to_string);
    keys.dedup();
    Footprint {
        name: name.to_owned(),
        publisher: values.text("Publisher"),
        folders,
        keys,
    }
}

/// The folder of a `DisplayIcon` value: a quoted path with an icon index, or a bare path.
fn icon_folder(icon: &str) -> Option<String> {
    let icon = icon.trim();
    let path = if let Some(rest) = icon.strip_prefix('"') {
        rest.split_once('"')?.0
    } else {
        match icon.rsplit_once(',') {
            Some((path, index)) if index.trim().trim_start_matches('-').parse::<u32>().is_ok() => {
                path
            }
            _ => icon,
        }
    };
    path.trim()
        .rsplit_once(['\\', '/'])
        .map(|(folder, _)| folder.to_owned())
        .filter(|folder| !folder.is_empty())
}

/// A registry key written as text (`HKCU\Software\X`), when it is in a hive Volt looks at.
fn registry_path(text: &str) -> Option<RegistryKeyRef> {
    let (root, rest) = text.trim().split_once('\\')?;
    let hive = match root.to_ascii_uppercase().as_str() {
        "HKCU" | "HKEY_CURRENT_USER" => RegistryHive::CurrentUser,
        "HKLM" | "HKEY_LOCAL_MACHINE" => RegistryHive::LocalMachine,
        _ => return None,
    };
    let rest = rest.trim_matches('\\');
    (!rest.is_empty() && rest.len() <= 260 && !rest.chars().any(char::is_control))
        .then(|| RegistryKeyRef::new(hive, rest, RegistryView::Native))
}
