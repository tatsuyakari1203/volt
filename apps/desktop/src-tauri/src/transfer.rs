//! Tweak drift and the configuration file, at the IPC boundary.
//!
//! The rules live in `volt_core::tweak`: which tweaks drifted ([`volt_core::TweakEngine::drifted`]),
//! what accepting the new state does ([`volt_core::TweakEngine::accept`]), the file format and the
//! import plan ([`volt_core::ConfigFile`], [`volt_core::TweakEngine::plan_import`]). This module
//! adds what only the shell knows: the settings that go in the file, the file dialogs and the
//! transfer types.
//!
//! **A file is data.** An import reads tweak ids and the keys of the settings listed below, and
//! nothing else. A tweak is applied from this machine's catalogue through the engine, so every
//! change is journaled and can be undone. A setting value is parsed into its typed form and
//! normalised before it is stored, and a value that does not parse is refused.
//!
//! **Left out of the file**, because they only make sense on the machine they were made on: the
//! folders added for cleaning rules (`clean.chosen_folders`, machine paths), the saved process rules
//! (`process.rules`, their cores and priorities fit this CPU and this set of programs) and the
//! language (`ui.language`, it follows Windows by default and is chosen in Settings).
//!
//! The file is read and written from Rust only, through native dialogs: the UI has no file system
//! access. A previewed import waits in [`PENDING_IMPORT`] until the user confirms it.

use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::Path;
use std::sync::{Mutex, PoisonError};

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, Manager as _, State};
use tauri_plugin_dialog::DialogExt as _;
use volt_core::{
    ApplyOutcome, BOOST_SETTINGS_KEY, BoostConfig, Clock, ConfigFile, EXCLUSIONS_SETTINGS_KEY,
    ExclusionList, ImportAction, ImportSkip, MAX_CONFIG_BYTES, Phrase, Source, SystemClock,
    TweakImport,
};

use crate::IpcError;
use crate::api::{
    TweakStateView, TweakStepView, apply_failure_detail, state_view, step_view,
    warn_if_history_incomplete, write_export,
};
use crate::commands::blocking;
use crate::phrase::PhraseView;
use crate::wiring::{AppCore, AppState};

// ---- drift -----------------------------------------------------------------------------------

/// A tweak Volt applied that something else has changed since.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DriftView {
    /// The tweak id.
    pub id: String,
    /// What the system holds now: `notApplied` (nothing of it is left) or `partial`.
    pub state: TweakStateView,
    /// Every step with its value now and the value the tweak sets, so the user sees what changed.
    pub steps: Vec<TweakStepView>,
}

/// Result of [`accept_tweak_state`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AcceptResult {
    /// The tweak id.
    pub tweak: String,
    /// How many undo records were set aside (they stay in the history, but cannot be undone).
    pub accepted: u32,
    /// The state read again.
    pub state: TweakStateView,
}

/// The tweaks Volt applied whose state no longer matches. Read-only.
pub fn list_tweak_drift_of(core: &AppCore) -> Result<Vec<DriftView>, IpcError> {
    let _exclusive = core.exclusive();
    core.engine
        .drifted()
        .into_iter()
        .map(|drift| {
            let steps = core
                .engine
                .describe(&drift.tweak)?
                .into_iter()
                .map(step_view)
                .collect();
            Ok(DriftView {
                state: state_view(Ok(drift.state)),
                id: drift.tweak,
                steps,
            })
        })
        .collect()
}

/// Accepts the state a drifted tweak is in: Volt stops treating the tweak as applied and its undo
/// records can no longer be used. Changes nothing on the system. Journaled in the sense that the
/// records are kept, only marked.
pub fn accept_tweak_state_of(core: &AppCore, id: &str) -> Result<AcceptResult, IpcError> {
    let _exclusive = core.exclusive();
    let accepted = core.engine.accept(id)?;
    Ok(AcceptResult {
        tweak: id.to_owned(),
        accepted: u32::try_from(accepted).unwrap_or(u32::MAX),
        state: state_view(core.engine.state(id)),
    })
}

// ---- settings in the file ----------------------------------------------------------------------

/// The settings the file carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Portable {
    Boost,
    Exclusions,
}

impl Portable {
    const ALL: [Self; 2] = [Self::Boost, Self::Exclusions];

    const fn key(self) -> &'static str {
        match self {
            Self::Boost => BOOST_SETTINGS_KEY,
            Self::Exclusions => EXCLUSIONS_SETTINGS_KEY,
        }
    }

    fn of(key: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|candidate| candidate.key() == key)
    }
}

/// What an import does with one setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum SettingStatus {
    /// The file's value differs from this machine's: it will be stored.
    Change,
    /// This machine already has it.
    Same,
    /// The value in the file is not usable, so it is left out.
    Invalid,
}

/// One setting of a file and what an import does with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SettingImportView {
    /// The settings key, for example `boost.config`.
    pub key: String,
    /// What happens.
    pub status: SettingStatus,
    /// What this machine has now, when the value is replaced, in English.
    pub current: Option<String>,
    /// `current` as a code and parameters.
    pub current_phrase: Option<PhraseView>,
    /// What it will hold, in English.
    pub new: Option<String>,
    /// `new` as a code and parameters.
    pub new_phrase: Option<PhraseView>,
}

/// A setting ready to be stored, with how it is shown.
struct SettingPlan {
    key: Portable,
    status: SettingStatus,
    current: Option<Phrase>,
    new: Option<Phrase>,
    write: Option<serde_json::Value>,
}

fn boost_phrase(config: &BoostConfig) -> Phrase {
    let areas = if config.areas.is_empty() {
        Phrase::new("transfer.nothing")
    } else {
        Phrase::new("transfer.list").list(
            "items",
            ", ",
            config.areas.iter().map(|area| area.phrase()).collect(),
        )
    };
    Phrase::new("transfer.boost").phrase("areas", areas).phrase(
        "bin",
        Phrase::new(if config.move_temp_to_recycle_bin {
            "transfer.on"
        } else {
            "transfer.off"
        }),
    )
}

/// The settings of this machine that go in a file.
fn current_settings(core: &AppCore) -> Result<BTreeMap<String, serde_json::Value>, IpcError> {
    let mut settings = BTreeMap::new();
    let boost = BoostConfig::load(core.settings.as_ref());
    settings.insert(Portable::Boost.key().to_owned(), to_value(&boost)?);
    let exclusions = ExclusionList::load(core.settings.as_ref())?;
    if !exclusions.is_empty() {
        settings.insert(
            Portable::Exclusions.key().to_owned(),
            to_value(&exclusions)?,
        );
    }
    Ok(settings)
}

fn to_value<T: Serialize>(value: &T) -> Result<serde_json::Value, IpcError> {
    serde_json::to_value(value).map_err(|error| IpcError::Other {
        message: format!("a setting could not be written: {error}"),
    })
}

fn plan_setting(
    core: &AppCore,
    key: Portable,
    value: &serde_json::Value,
) -> Result<SettingPlan, IpcError> {
    let invalid = || SettingPlan {
        key,
        status: SettingStatus::Invalid,
        current: None,
        new: None,
        write: None,
    };
    match key {
        Portable::Boost => {
            let Ok(mut incoming) = serde_json::from_value::<BoostConfig>(value.clone()) else {
                return Ok(invalid());
            };
            let mut areas = Vec::new();
            for area in incoming.areas {
                if !areas.contains(&area) {
                    areas.push(area);
                }
            }
            incoming.areas = areas;
            let current = BoostConfig::load(core.settings.as_ref());
            if incoming == current {
                return Ok(SettingPlan {
                    status: SettingStatus::Same,
                    ..invalid()
                });
            }
            Ok(SettingPlan {
                key,
                status: SettingStatus::Change,
                current: Some(boost_phrase(&current)),
                new: Some(boost_phrase(&incoming)),
                write: Some(to_value(&incoming)?),
            })
        }
        Portable::Exclusions => {
            let Ok(names) = serde_json::from_value::<Vec<String>>(value.clone()) else {
                return Ok(invalid());
            };
            // Names are added to the list, never removed: a longer list protects more programs.
            let mut merged = ExclusionList::load(core.settings.as_ref())?;
            let mut added = Vec::new();
            for name in &names {
                if let Ok(normal) = ExclusionList::normalize(name)
                    && merged.add(&normal).unwrap_or(false)
                {
                    added.push(normal);
                }
            }
            if added.is_empty() {
                return Ok(SettingPlan {
                    status: SettingStatus::Same,
                    ..invalid()
                });
            }
            Ok(SettingPlan {
                key,
                status: SettingStatus::Change,
                current: None,
                new: Some(
                    Phrase::new("transfer.exclusions")
                        .count("count", added.len())
                        .text("names", added.join(", ")),
                ),
                write: Some(to_value(&merged)?),
            })
        }
    }
}

fn store_setting(core: &AppCore, key: Portable, value: &serde_json::Value) -> Result<(), IpcError> {
    core.settings.set(key.key(), value)?;
    Ok(())
}

// ---- export ------------------------------------------------------------------------------------

/// A configuration, as text for a file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ConfigExport {
    /// Suggested file name.
    pub file_name: String,
    /// The file's text.
    pub text: String,
    /// How many tweaks it names.
    pub tweaks: u32,
    /// How many settings it holds.
    pub settings: u32,
}

/// How [`export_configuration`] ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ConfigExportResult {
    /// The file was written.
    Saved {
        /// Where.
        path: String,
        /// How many tweaks it names.
        tweaks: u32,
        /// How many settings it holds.
        settings: u32,
    },
    /// The user closed the dialog. Nothing was written.
    Cancelled,
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Builds the configuration file of this machine. Read-only.
pub fn export_configuration_text(core: &AppCore) -> Result<ConfigExport, IpcError> {
    let _exclusive = core.exclusive();
    let tweaks = core.engine.export_tweaks();
    let settings = current_settings(core)?;
    let (n_tweaks, n_settings) = (tweaks.len(), settings.len());
    let text = ConfigFile::new(tweaks, settings)
        .to_text()
        .map_err(|error| IpcError::Other {
            message: error.to_string(),
        })?;
    let iso = SystemClock.now().to_iso8601();
    Ok(ConfigExport {
        file_name: format!(
            "volt-configuration-{}.json",
            iso.get(..10).unwrap_or("export")
        ),
        text,
        tweaks: count(n_tweaks),
        settings: count(n_settings),
    })
}

// ---- import ------------------------------------------------------------------------------------

/// Why a tweak of the file is left out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum SkipView {
    /// This version of Volt has no such tweak.
    Unknown,
    /// It needs administrator rights, which Volt does not have in this session.
    NeedsAdministrator,
    /// This machine lacks something it changes, or its state cannot be read.
    Unavailable,
}

/// What an import does with one tweak.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ImportActionView {
    /// It will be applied: these are the changes.
    Apply {
        /// Every change, with the value now and after.
        steps: Vec<TweakStepView>,
    },
    /// Already in place here; nothing to do.
    AlreadyApplied,
    /// Left out.
    Skip {
        /// Why.
        reason: SkipView,
        /// More words for `unavailable`.
        detail: Option<String>,
    },
}

/// One tweak of a file and what an import does with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TweakImportView {
    /// The id in the file.
    pub id: String,
    /// The tweak's name in this catalogue; `null` for an id Volt does not know.
    pub title: Option<String>,
    /// What happens to it.
    pub action: ImportActionView,
}

/// Exactly what an import would change on this machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ImportPreview {
    /// The tweaks of the file, each once, in file order.
    pub tweaks: Vec<TweakImportView>,
    /// The settings of the file that Volt knows.
    pub settings: Vec<SettingImportView>,
    /// How many entries of `settings` in the file were ignored because Volt does not know them.
    pub ignored_settings: u32,
}

/// How [`preview_configuration_import`] ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ImportPreviewResult {
    /// A file was read and nothing was changed; the user decides what to do with the preview.
    #[serde(rename_all = "camelCase")]
    Preview {
        /// The file's name.
        file_name: String,
        /// What an import would do.
        preview: ImportPreview,
    },
    /// The user closed the dialog.
    Cancelled,
}

/// How one tweak of an import ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum ImportStatus {
    /// Applied, through the journal.
    Applied,
    /// Already in place.
    AlreadyApplied,
    /// A step failed and the system is as it was.
    RolledBack,
    /// A step failed and the system may differ from before.
    NeedsAttention,
    /// Left out (see the preview), or no longer applicable.
    Skipped,
    /// It could not be applied at all.
    Failed,
}

/// How one tweak of an import ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TweakImportResult {
    /// The tweak id.
    pub id: String,
    /// The tweak's name; `null` for an id Volt does not know.
    pub title: Option<String>,
    /// How it ended.
    pub status: ImportStatus,
    /// Why it failed, in words.
    pub detail: Option<String>,
}

/// What an import did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    /// Every tweak of the file.
    pub tweaks: Vec<TweakImportResult>,
    /// How many settings were stored.
    pub settings_stored: u32,
    /// How many settings could not be stored.
    pub settings_failed: u32,
}

fn title_of(core: &AppCore, id: &str) -> Option<String> {
    core.engine
        .catalogue()
        .get(id)
        .map(|tweak| tweak.title.clone())
}

fn tweak_view(core: &AppCore, item: TweakImport) -> TweakImportView {
    let action = match item.action {
        ImportAction::Apply(previews) => ImportActionView::Apply {
            steps: previews
                .into_iter()
                .map(|preview| TweakStepView {
                    subject: preview.subject,
                    current: preview.current,
                    target: preview.target,
                    subject_phrase: (&preview.subject_phrase).into(),
                    current_phrase: (&preview.current_phrase).into(),
                    target_phrase: (&preview.target_phrase).into(),
                    at_target: false,
                })
                .collect(),
        },
        ImportAction::AlreadyApplied => ImportActionView::AlreadyApplied,
        ImportAction::Skip(ImportSkip::NeedsAdministrator) => ImportActionView::Skip {
            reason: SkipView::NeedsAdministrator,
            detail: None,
        },
        ImportAction::Skip(ImportSkip::Unavailable(detail)) => ImportActionView::Skip {
            reason: SkipView::Unavailable,
            detail: Some(detail),
        },
        // `ImportSkip` and `ImportAction` are non-exhaustive: `Unknown`, and anything new, is
        // left out.
        _ => ImportActionView::Skip {
            reason: SkipView::Unknown,
            detail: None,
        },
    };
    TweakImportView {
        title: title_of(core, &item.id),
        id: item.id,
        action,
    }
}

/// Reads a configuration file's text and says what importing it would do here. Read-only.
pub fn preview_import(core: &AppCore, file: &ConfigFile) -> Result<ImportPreview, IpcError> {
    let _exclusive = core.exclusive();
    let tweaks = core
        .engine
        .plan_import(&file.tweaks)
        .into_iter()
        .map(|item| tweak_view(core, item))
        .collect();
    let mut settings = Vec::new();
    let mut ignored = 0;
    for (key, value) in &file.settings {
        match Portable::of(key) {
            Some(known) => {
                let plan = plan_setting(core, known, value)?;
                settings.push(SettingImportView {
                    key: known.key().to_owned(),
                    status: plan.status,
                    current: plan.current.as_ref().map(Phrase::english),
                    current_phrase: plan.current.as_ref().map(PhraseView::from),
                    new: plan.new.as_ref().map(Phrase::english),
                    new_phrase: plan.new.as_ref().map(PhraseView::from),
                });
            }
            None => ignored += 1,
        }
    }
    Ok(ImportPreview {
        tweaks,
        settings,
        ignored_settings: ignored,
    })
}

/// Applies what the preview of `file` showed, read again now: each tweak through the engine (one
/// journal batch each, so each can be undone), then the settings. Whatever stopped being
/// applicable since the preview is skipped.
pub fn apply_import(core: &AppCore, file: &ConfigFile) -> Result<ImportResult, IpcError> {
    let _exclusive = core.exclusive();
    let mut tweaks = Vec::new();
    for item in core.engine.plan_import(&file.tweaks) {
        let title = title_of(core, &item.id);
        let (status, detail) = match item.action {
            ImportAction::Apply(_) => {
                let failures_before = core.engine.activity_write_failures();
                let result = core.engine.apply(&item.id, Source::Manual);
                warn_if_history_incomplete(core, failures_before, "importing a tweak");
                match result {
                    Ok(report) => match report.outcome {
                        ApplyOutcome::Applied => (ImportStatus::Applied, None),
                        ApplyOutcome::AlreadyApplied => (ImportStatus::AlreadyApplied, None),
                        ApplyOutcome::RolledBack => {
                            (ImportStatus::RolledBack, apply_failure_detail(&report))
                        }
                        _ => (ImportStatus::NeedsAttention, apply_failure_detail(&report)),
                    },
                    Err(error) => (ImportStatus::Failed, Some(error.to_string())),
                }
            }
            ImportAction::AlreadyApplied => (ImportStatus::AlreadyApplied, None),
            _ => (ImportStatus::Skipped, None),
        };
        tweaks.push(TweakImportResult {
            id: item.id,
            title,
            status,
            detail,
        });
    }
    let (mut stored, mut failed) = (0, 0);
    for (key, value) in &file.settings {
        let Some(known) = Portable::of(key) else {
            continue;
        };
        let plan = plan_setting(core, known, value)?;
        if let Some(write) = plan.write.filter(|_| plan.status == SettingStatus::Change) {
            match store_setting(core, plan.key, &write) {
                Ok(()) => stored += 1,
                Err(error) => {
                    log::warn!(
                        "a setting from a configuration file could not be stored: {error:?}"
                    );
                    failed += 1;
                }
            }
        }
    }
    Ok(ImportResult {
        tweaks,
        settings_stored: stored,
        settings_failed: failed,
    })
}

// ---- files and commands --------------------------------------------------------------------------

/// The import the user has previewed and not yet confirmed. Only the confirm step reads it, and
/// it can only ever hold a parsed [`ConfigFile`]: the UI never names a path or sends text.
static PENDING_IMPORT: Mutex<Option<ConfigFile>> = Mutex::new(None);

fn pending() -> std::sync::MutexGuard<'static, Option<ConfigFile>> {
    PENDING_IMPORT
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// Reads a configuration file: at most [`MAX_CONFIG_BYTES`], as UTF-8 text, and it must be a
/// Volt configuration of a version this build reads.
pub fn read_config_file(path: &Path) -> Result<ConfigFile, IpcError> {
    let mut text = String::new();
    std::fs::File::open(path)
        .and_then(|file| {
            file.take(u64::try_from(MAX_CONFIG_BYTES).unwrap_or(u64::MAX) + 1)
                .read_to_string(&mut text)
        })
        .map_err(|error| {
            let message = format!("could not read {}: {error}", path.display());
            match error.kind() {
                std::io::ErrorKind::PermissionDenied => IpcError::AccessDenied { message },
                std::io::ErrorKind::NotFound => IpcError::NotFound { message },
                std::io::ErrorKind::InvalidData => IpcError::InvalidInput {
                    message: "this is not a Volt configuration file".to_owned(),
                },
                _ => IpcError::Other { message },
            }
        })?;
    ConfigFile::parse(&text).map_err(|error| IpcError::InvalidInput {
        message: error.to_string(),
    })
}

/// Tauri command: the tweaks Volt applied that Windows or another program has changed since.
/// Reads the system only.
#[tauri::command]
#[specta::specta]
pub(crate) async fn list_tweak_drift(
    state: State<'_, AppState>,
) -> Result<Vec<DriftView>, IpcError> {
    blocking(&state, list_tweak_drift_of).await
}

/// Tauri command: keeps the state a drifted tweak is in. Volt stops treating the tweak as applied
/// and its undo records can no longer be used (they stay in the history). Changes nothing on the
/// system. To put the tweak back, apply it again.
#[tauri::command]
#[specta::specta]
pub(crate) async fn accept_tweak_state(
    state: State<'_, AppState>,
    id: String,
) -> Result<AcceptResult, IpcError> {
    blocking(&state, move |core| accept_tweak_state_of(core, &id)).await
}

/// Tauri command: asks where to save the configuration (a native save dialog) and writes it. The
/// file lists the tweaks Volt applied that are still in place and the settings that make sense on
/// another machine. Closing the dialog is `cancelled`.
#[tauri::command]
#[specta::specta]
pub(crate) async fn export_configuration(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<ConfigExportResult, IpcError> {
    blocking(&state, move |core| {
        let export = export_configuration_text(core)?;
        let mut dialog = app
            .dialog()
            .file()
            .set_title("Save the Volt configuration")
            .set_file_name(&export.file_name)
            .add_filter("Volt configuration", &["json"]);
        if let Some(window) = app.get_webview_window("main") {
            dialog = dialog.set_parent(&window);
        }
        let Some(chosen) = dialog.blocking_save_file() else {
            return Ok(ConfigExportResult::Cancelled);
        };
        let path = chosen.into_path().map_err(|error| IpcError::Other {
            message: format!("the chosen location cannot be used: {error}"),
        })?;
        write_export(&path, &export.text)?;
        Ok(ConfigExportResult::Saved {
            path: path.display().to_string(),
            tweaks: export.tweaks,
            settings: export.settings,
        })
    })
    .await
}

/// Tauri command: asks for a configuration file (a native open dialog), reads it and says exactly
/// what importing it would change here. Changes nothing; [`apply_configuration_import`] applies
/// what was shown, after the user confirms.
#[tauri::command]
#[specta::specta]
pub(crate) async fn preview_configuration_import(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<ImportPreviewResult, IpcError> {
    blocking(&state, move |core| {
        let mut dialog = app
            .dialog()
            .file()
            .set_title("Open a Volt configuration")
            .add_filter("Volt configuration", &["json"]);
        if let Some(window) = app.get_webview_window("main") {
            dialog = dialog.set_parent(&window);
        }
        let Some(chosen) = dialog.blocking_pick_file() else {
            return Ok(ImportPreviewResult::Cancelled);
        };
        let path = chosen.into_path().map_err(|error| IpcError::Other {
            message: format!("the chosen file cannot be used: {error}"),
        })?;
        let file = read_config_file(&path)?;
        let preview = preview_import(core, &file)?;
        *pending() = Some(file);
        Ok(ImportPreviewResult::Preview {
            file_name: path
                .file_name()
                .map_or_else(String::new, |name| name.to_string_lossy().into_owned()),
            preview,
        })
    })
    .await
}

/// Tauri command: applies the import that was previewed last and confirmed. The change to each
/// tweak is journaled, so it can be undone. Fails with `invalidInput` when no preview is waiting.
#[tauri::command]
#[specta::specta]
pub(crate) async fn apply_configuration_import(
    state: State<'_, AppState>,
) -> Result<ImportResult, IpcError> {
    blocking(&state, move |core| {
        let file = pending().take().ok_or_else(|| IpcError::InvalidInput {
            message: "no configuration is waiting to be imported".to_owned(),
        })?;
        apply_import(core, &file)
    })
    .await
}
