//! Profiles at the IPC boundary: the transfer types, the commands behind the Profiles page and the
//! Undo page's "undo this switch", and the look at running apps that activates a profile when its
//! app starts.
//!
//! The rules live in `volt_core::profile`; this module translates them into the shell's own types
//! (see [`crate::api`] for why the shell owns its transfer types). The timer that calls
//! [`poll_of`] is in [`crate::agent`].
//!
//! Every change a switch makes goes through the tweak engine, so each tweak is its own journal
//! batch and the same lock ([`AppCore::exclusive`]) keeps a switch from overlapping a tweak, a
//! Boost or a clean.

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri_specta::Event;
use volt_core::profile::{MAX_NAME_CHARS, MAX_PROFILES, TRIGGER_POLL_SECS};
use volt_core::{
    AppTrigger, AutoEvent, AutomationOutcome, LeftOut, Phrase, Profile, ProfileAutomation,
    ProfileError, ProfileTweak, Profiles, Source, StepOutcome, SwitchAction, SwitchPlan,
    SwitchReport, Timestamp, is_automatic_rule,
};

use crate::IpcError;
use crate::api::{
    ActivitySource, RiskLevel, TweakStepView, UndoPreviewItem, apply_failure_detail,
    revert_failure_detail, risk_view, undo_preview, warn_if_history_incomplete,
};
use crate::automation::{AutomationConfigView, MemoryRuleView, PauseView, ScheduleView};
use crate::commands::blocking;
use crate::language::{self, Lang};
use crate::phrase::PhraseView;
use crate::wiring::{AppCore, AppState};

// ---- the profiles -------------------------------------------------------------------------------

/// A tweak of a profile and the state the profile wants it in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProfileTweakView {
    /// The tweak id.
    pub id: String,
    /// `true`: applied. `false`: turned off when Volt applied it.
    pub enabled: bool,
}

/// The app that activates a profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AppTriggerView {
    /// Executable name, such as `game.exe`.
    pub exe: String,
    /// Whether the earlier profile returns when the app exits.
    pub return_on_exit: bool,
}

/// The automation settings a profile sets; a part that is `null` is left as it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProfileAutomationView {
    /// The memory rule.
    pub memory: Option<MemoryRuleView>,
    /// The cleaning schedule.
    pub cleaning: Option<ScheduleView>,
}

/// A profile. Also what the UI sends to save one (an empty `id` is a new profile).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProfileView {
    /// Stable id; empty for a profile that is not saved yet.
    pub id: String,
    /// The name shown to the user.
    pub name: String,
    /// The tweaks, in the order they are applied.
    pub tweaks: Vec<ProfileTweakView>,
    /// The automation settings it sets.
    pub automation: ProfileAutomationView,
    /// The app that activates it.
    pub trigger: Option<AppTriggerView>,
}

impl ProfileView {
    fn from_core(profile: &Profile) -> Self {
        Self {
            id: profile.id.clone(),
            name: profile.name.clone(),
            tweaks: profile
                .tweaks
                .iter()
                .map(|tweak| ProfileTweakView {
                    id: tweak.id.clone(),
                    enabled: tweak.enabled,
                })
                .collect(),
            automation: ProfileAutomationView {
                memory: profile
                    .automation
                    .memory
                    .as_ref()
                    .map(MemoryRuleView::from_core),
                cleaning: profile
                    .automation
                    .cleaning
                    .as_ref()
                    .map(ScheduleView::from_core),
            },
            trigger: profile.trigger.as_ref().map(|trigger| AppTriggerView {
                exe: trigger.exe.clone(),
                return_on_exit: trigger.return_on_exit,
            }),
        }
    }

    fn to_core(&self) -> Profile {
        Profile {
            id: self.id.clone(),
            name: self.name.clone(),
            tweaks: self
                .tweaks
                .iter()
                .map(|tweak| ProfileTweak {
                    id: tweak.id.clone(),
                    enabled: tweak.enabled,
                })
                .collect(),
            automation: ProfileAutomation {
                memory: self.automation.memory.as_ref().map(MemoryRuleView::to_core),
                cleaning: self.automation.cleaning.as_ref().map(ScheduleView::to_core),
            },
            trigger: self.trigger.as_ref().map(|trigger| AppTrigger {
                exe: trigger.exe.clone(),
                return_on_exit: trigger.return_on_exit,
            }),
        }
    }
}

/// A tweak by id and name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TweakRefView {
    /// The tweak id.
    pub id: String,
    /// Its name.
    pub title: String,
}

/// The last switch, which the Undo page can put back as one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SwitchRecordView {
    /// When, in milliseconds since the Unix epoch.
    pub at: f64,
    /// Who started it.
    pub source: ActivitySource,
    /// The profile that was active before.
    pub from: Option<String>,
    /// The profile it switched to.
    pub to: Option<String>,
    /// The tweaks it turned on.
    pub applied: Vec<TweakRefView>,
    /// The tweaks it turned off.
    pub reverted: Vec<TweakRefView>,
    /// Whether it also changed the automation settings.
    pub automation_changed: bool,
    /// Whether it finished. A switch that stopped part-way can still be undone.
    pub complete: bool,
}

/// An automatic switch that a pause skipped.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AutoSkipView {
    /// When, in milliseconds since the Unix epoch.
    pub at: f64,
    /// Why.
    pub reason: PauseView,
}

/// The profiles and which one is active.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProfilesView {
    /// The profiles, in the order the user keeps them.
    pub profiles: Vec<ProfileView>,
    /// Whether the user has not saved a list yet, so these are the built-in suggestions.
    pub suggested: bool,
    /// The active profile (`null` until the user chooses one).
    pub active: Option<String>,
    /// The last switch, when it can still be undone.
    pub last_switch: Option<SwitchRecordView>,
    /// The last automatic switch that a pause skipped.
    pub last_skip: Option<AutoSkipView>,
    /// Whether Volt is looking at running apps for a profile right now.
    pub watching: bool,
    /// Seconds between two looks at the running apps while a profile names an app.
    pub poll_seconds: u32,
    /// The most profiles that are kept.
    pub max_profiles: u32,
    /// The longest profile name, in characters.
    pub max_name_chars: u32,
}

fn millis(at: Timestamp) -> f64 {
    #[allow(
        clippy::cast_precision_loss,
        reason = "a timestamp in milliseconds is far below 2^53"
    )]
    let value = at.as_unix_millis() as f64;
    value
}

fn title_of(core: &AppCore, id: &str) -> String {
    core.engine
        .catalogue()
        .get(id)
        .map_or_else(|| id.to_owned(), |tweak| tweak.title.clone())
}

fn refs(core: &AppCore, ids: &[String]) -> Vec<TweakRefView> {
    ids.iter()
        .map(|id| TweakRefView {
            id: id.clone(),
            title: title_of(core, id),
        })
        .collect()
}

fn error_of(error: &ProfileError) -> IpcError {
    let phrase = error.phrase();
    match error {
        ProfileError::Unknown(_) | ProfileError::NothingToUndo => IpcError::not_found(&phrase),
        ProfileError::Store(_) => IpcError::other(&phrase),
        _ => IpcError::invalid(&phrase),
    }
}

/// The profiles and the state, for the page.
pub fn list_profiles_of(core: &AppCore) -> Result<ProfilesView, IpcError> {
    let profiles = core.profiles()?;
    let state = profiles.state();
    Ok(ProfilesView {
        suggested: profiles.saved().is_none(),
        profiles: profiles.list().iter().map(ProfileView::from_core).collect(),
        active: state.active,
        last_switch: state.last_switch.map(|record| SwitchRecordView {
            at: millis(record.at),
            source: match record.source {
                Source::Manual => ActivitySource::Manual,
                Source::Automation => ActivitySource::Automation,
                _ => ActivitySource::Other,
            },
            from: record.from,
            to: record.to,
            applied: refs(core, &record.applied),
            reverted: refs(core, &record.reverted),
            automation_changed: record.automation_before.is_some(),
            complete: record.complete,
        }),
        last_skip: state.last_skip.map(|skip| AutoSkipView {
            at: millis(skip.at),
            reason: PauseView::from_core(skip.reason),
        }),
        watching: profiles.watching(),
        poll_seconds: u32::try_from(TRIGGER_POLL_SECS).unwrap_or(u32::MAX),
        max_profiles: u32::try_from(MAX_PROFILES).unwrap_or(u32::MAX),
        max_name_chars: u32::try_from(MAX_NAME_CHARS).unwrap_or(u32::MAX),
    })
}

/// Saves a profile (a new one has an empty id). A cleaning rule the schedule part names must be
/// one that can run on its own, like the schedule on the Automation card.
pub fn save_profile_of(core: &AppCore, profile: &ProfileView) -> Result<ProfilesView, IpcError> {
    let profiles = core.profiles()?;
    let profile = profile.to_core();
    if let Some(schedule) = &profile.automation.cleaning {
        check_rules(core, &schedule.rule_ids)?;
    }
    let _exclusive = core.exclusive();
    profiles.save(profile).map_err(|error| error_of(&error))?;
    list_profiles_of(core)
}

fn check_rules(core: &AppCore, ids: &[String]) -> Result<(), IpcError> {
    let Some(engine) = core.clean_engine() else {
        return Ok(());
    };
    for id in ids {
        match engine.rules().get(id) {
            Some(rule) if is_automatic_rule(rule) => {}
            Some(rule) => {
                return Err(IpcError::invalid(
                    &Phrase::new("err.automation.notAutomatic").text("name", rule.title.clone()),
                ));
            }
            None => {
                return Err(IpcError::not_found(
                    &Phrase::new("err.ruleNotInScan").text("id", id),
                ));
            }
        }
    }
    Ok(())
}

/// Removes a profile. What it applied stays applied.
pub fn delete_profile_of(core: &AppCore, id: &str) -> Result<ProfilesView, IpcError> {
    let profiles = core.profiles()?;
    let _exclusive = core.exclusive();
    profiles.delete(id).map_err(|error| error_of(&error))?;
    list_profiles_of(core)
}

// ---- the preview ---------------------------------------------------------------------------------

/// What a switch does to one tweak.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum SwitchActionView {
    /// Applied: the steps say what changes.
    TurnOn,
    /// Reverted from the undo journal: the restores say what is put back.
    TurnOff,
}

/// One tweak a switch changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SwitchItemView {
    /// The tweak id.
    pub tweak: String,
    /// Its name.
    pub title: String,
    /// How risky it is. Caution and Advanced tweaks are named in the preview.
    pub risk: RiskLevel,
    /// What happens to it.
    pub action: SwitchActionView,
    /// For a tweak that is turned on: every change, with the value now and after.
    pub steps: Vec<TweakStepView>,
    /// For a tweak that is turned off: what is put back.
    pub restores: Vec<UndoPreviewItem>,
}

/// Why a tweak of a profile is left alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum LeftOutView {
    /// This version of Volt has no such tweak.
    Unknown,
    /// It needs administrator rights, which a profile does not use.
    NeedsAdministrator,
    /// It cannot be applied here, or its state cannot be read.
    Unavailable,
    /// It is on because Windows or the user set it: Volt has no earlier value to put back.
    NotAppliedByVolt,
}

/// A tweak of a profile that a switch leaves alone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct NotIncludedView {
    /// The tweak id.
    pub tweak: String,
    /// Its name, when this version knows it.
    pub title: Option<String>,
    /// Why.
    pub reason: LeftOutView,
    /// More words for `unavailable`, in English.
    pub detail: Option<String>,
}

/// The automation settings before and after a switch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AutomationChangeView {
    /// What is saved now.
    pub before: AutomationConfigView,
    /// What the switch saves.
    pub after: AutomationConfigView,
}

/// Exactly what a switch would change right now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SwitchPreviewView {
    /// The active profile now.
    pub from: Option<String>,
    /// The profile to switch to (`null`: turn the active profile off).
    pub to: Option<String>,
    /// The tweaks that change: turned off first, then on.
    pub items: Vec<SwitchItemView>,
    /// Tweaks left alone, with the reason.
    pub left_out: Vec<NotIncludedView>,
    /// How many tweaks the profile wants that are in place already.
    pub in_place: u32,
    /// The automation change, when there is one.
    pub automation: Option<AutomationChangeView>,
    /// Whether the pause rules would skip an automatic switch right now (a manual one is never
    /// skipped).
    pub paused: Option<PauseView>,
}

fn step_of(preview: volt_core::Preview) -> TweakStepView {
    TweakStepView {
        subject: preview.subject,
        current: preview.current,
        target: preview.target,
        subject_phrase: (&preview.subject_phrase).into(),
        current_phrase: (&preview.current_phrase).into(),
        target_phrase: (&preview.target_phrase).into(),
        at_target: false,
    }
}

fn preview_view(core: &AppCore, plan: SwitchPlan) -> Result<SwitchPreviewView, IpcError> {
    let mut items = Vec::new();
    for item in plan.items {
        let (action, steps, restores) = match item.action {
            SwitchAction::TurnOn(previews) => (
                SwitchActionView::TurnOn,
                previews.into_iter().map(step_of).collect(),
                Vec::new(),
            ),
            SwitchAction::TurnOff => {
                let applied = core.engine.applied_entries(&item.tweak)?;
                (
                    SwitchActionView::TurnOff,
                    Vec::new(),
                    undo_preview(core, &applied)?.items,
                )
            }
        };
        items.push(SwitchItemView {
            tweak: item.tweak,
            title: item.title,
            risk: risk_view(item.risk),
            action,
            steps,
            restores,
        });
    }
    Ok(SwitchPreviewView {
        from: plan.from,
        to: plan.to,
        items,
        left_out: plan
            .left_out
            .into_iter()
            .map(|left| {
                let (reason, detail) = match left.reason {
                    LeftOut::NeedsAdministrator => (LeftOutView::NeedsAdministrator, None),
                    LeftOut::Unavailable(detail) => (LeftOutView::Unavailable, Some(detail)),
                    LeftOut::NotAppliedByVolt => (LeftOutView::NotAppliedByVolt, None),
                    LeftOut::Unknown => (LeftOutView::Unknown, None),
                };
                NotIncludedView {
                    tweak: left.tweak,
                    title: left.title,
                    reason,
                    detail,
                }
            })
            .collect(),
        in_place: u32::try_from(plan.in_place).unwrap_or(u32::MAX),
        automation: plan.automation.map(|change| AutomationChangeView {
            before: AutomationConfigView::from_core(&change.before),
            after: AutomationConfigView::from_core(&change.after),
        }),
        paused: None,
    })
}

/// What switching to `id` (`None`: turning the active profile off) would change. Read-only.
pub fn preview_switch_of(core: &AppCore, id: Option<&str>) -> Result<SwitchPreviewView, IpcError> {
    let profiles = core.profiles()?;
    let automation = core.automation()?;
    let _exclusive = core.exclusive();
    let plan = profiles
        .plan(id, automation)
        .map_err(|error| error_of(&error))?;
    let mut view = preview_view(core, plan)?;
    view.paused = automation
        .pause_now(core.clean_engine())
        .map(PauseView::from_core);
    Ok(view)
}

// ---- the result ----------------------------------------------------------------------------------

/// How one tweak of a switch ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum StepStatusView {
    /// Changed.
    Done,
    /// Nothing was left to change.
    Nothing,
    /// The change failed and the tweak is as it was.
    RolledBack,
    /// The change failed and the tweak may differ from before.
    NeedsAttention,
    /// The tweak could not be read or changed.
    Failed,
    /// The switch stopped before it got here.
    NotAttempted,
}

/// One tweak of a switch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SwitchStepView {
    /// The tweak id.
    pub tweak: String,
    /// Its name.
    pub title: String,
    /// `true` when the step turned the tweak on, `false` when it turned it off.
    pub turned_on: bool,
    /// How it ended.
    pub status: StepStatusView,
    /// Why it failed, in English.
    pub detail: Option<String>,
    /// `detail` as a code and parameters.
    pub detail_phrase: Option<PhraseView>,
}

/// How the automation part of a switch ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum AutomationStatusKind {
    /// The profile sets none, or it was already so.
    NotChanged,
    /// Saved.
    Set,
    /// Could not be saved.
    Failed,
    /// The switch stopped before it got here.
    NotAttempted,
}

/// What a switch (or its undo) did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SwitchResultView {
    /// The active profile before.
    pub from: Option<String>,
    /// The profile the switch aimed at.
    pub to: Option<String>,
    /// Every tweak, in the order it was handled.
    pub steps: Vec<SwitchStepView>,
    /// The automation settings.
    pub automation: AutomationStatusKind,
    /// Whether everything was done. When it is not, the active profile did not change.
    pub complete: bool,
}

fn step_view(step: &volt_core::SwitchStep) -> SwitchStepView {
    let (status, detail): (_, Option<Phrase>) = match &step.outcome {
        StepOutcome::Done => (StepStatusView::Done, None),
        StepOutcome::Nothing => (StepStatusView::Nothing, None),
        StepOutcome::ApplyFailed(report) => (
            if report.outcome == volt_core::ApplyOutcome::RolledBack {
                StepStatusView::RolledBack
            } else {
                StepStatusView::NeedsAttention
            },
            apply_failure_detail(report),
        ),
        StepOutcome::RevertIncomplete(report) => (
            StepStatusView::NeedsAttention,
            revert_failure_detail(report),
        ),
        StepOutcome::Failed(error) => {
            let error = IpcError::from(error.clone());
            let phrase = error.phrase().cloned();
            return SwitchStepView {
                tweak: step.tweak.clone(),
                title: step.title.clone(),
                turned_on: step.turned_on,
                status: StepStatusView::Failed,
                detail: Some(error.message().to_owned()),
                detail_phrase: phrase,
            };
        }
        _ => (StepStatusView::NotAttempted, None),
    };
    SwitchStepView {
        tweak: step.tweak.clone(),
        title: step.title.clone(),
        turned_on: step.turned_on,
        status,
        detail: detail.as_ref().map(Phrase::english),
        detail_phrase: detail.as_ref().map(PhraseView::from),
    }
}

fn result_view(report: &SwitchReport) -> SwitchResultView {
    SwitchResultView {
        from: report.from.clone(),
        to: report.to.clone(),
        steps: report.steps.iter().map(step_view).collect(),
        automation: match report.automation {
            AutomationOutcome::NotChanged => AutomationStatusKind::NotChanged,
            AutomationOutcome::Set => AutomationStatusKind::Set,
            AutomationOutcome::Failed(_) => AutomationStatusKind::Failed,
            AutomationOutcome::NotAttempted => AutomationStatusKind::NotAttempted,
        },
        complete: report.complete,
    }
}

/// Switches to `id` (`None`: turns the active profile off): reverts and applies the difference,
/// each tweak through the journal. Reports precisely how it ended.
pub fn switch_of(core: &AppCore, id: Option<&str>) -> Result<SwitchResultView, IpcError> {
    let profiles = core.profiles()?;
    let automation = core.automation()?;
    let _exclusive = core.exclusive();
    let failures_before = core.engine.activity_write_failures();
    let result = profiles.switch(id, Source::Manual, automation);
    warn_if_history_incomplete(core, failures_before, "switching profile");
    result
        .map(|report| result_view(&report))
        .map_err(|error| error_of(&error))
}

/// Puts the last switch back as one action.
pub fn undo_switch_of(core: &AppCore) -> Result<SwitchResultView, IpcError> {
    let profiles = core.profiles()?;
    let automation = core.automation()?;
    let _exclusive = core.exclusive();
    let failures_before = core.engine.activity_write_failures();
    let result = profiles.undo_switch(automation);
    warn_if_history_incomplete(core, failures_before, "undoing a profile switch");
    result
        .map(|report| result_view(&report))
        .map_err(|error| error_of(&error))
}

// ---- automatic switches --------------------------------------------------------------------------

/// Sent to every window after an automatic switch, with the new state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type, Event)]
pub struct ProfilesUpdated(pub ProfilesView);

/// Whether the timer should look at the running apps at all.
#[must_use]
pub fn watching(core: &AppCore) -> bool {
    core.profiles().is_ok_and(Profiles::watching) && core.automation().is_ok()
}

/// One look at the running apps: activates the profile of an app that just started, brings the
/// earlier one back when it has exited. Holds the engine gate, so it never overlaps a tweak, a
/// Boost or a clean. Reads the process list only while [`watching`].
pub fn poll_of(core: &AppCore) -> Result<Vec<AutoEvent>, IpcError> {
    let profiles = core.profiles()?;
    let automation = core.automation()?;
    let _guard = core.exclusive();
    let failures_before = core.engine.activity_write_failures();
    let events = profiles.poll(&|| automation.pause_now(core.clean_engine()), automation);
    warn_if_history_incomplete(core, failures_before, "switching profile automatically");
    Ok(events)
}

/// The profile an event is about and whether it came on, for the tray.
fn event_subject(core: &AppCore, event: &AutoEvent) -> Option<(String, bool)> {
    let (report, on) = match event {
        AutoEvent::Activated(report) => (report, true),
        AutoEvent::Returned(report) => (report, false),
        _ => return None,
    };
    if !report.complete {
        return None;
    }
    let id = if on {
        report.to.as_ref()
    } else {
        report.from.as_ref()
    }?;
    let name = core
        .profiles()
        .ok()?
        .list()
        .into_iter()
        .find(|profile| &profile.id == id)
        .map(|profile| profile.name)?;
    Some((name, on))
}

/// A short line for the tray tooltip after an automatic switch, or `None` when none was made.
#[must_use]
pub fn tooltip_line(core: &AppCore, event: &AutoEvent, lang: Lang) -> Option<String> {
    let (name, on) = event_subject(core, event)?;
    let key = if on {
        "profile.tooltip.activated"
    } else {
        "profile.tooltip.returned"
    };
    Some(crate::phrase::write(
        &Phrase::new(key).text("name", name),
        lang,
    ))
}

/// The text of the notification after an automatic switch, or `None` when none was made.
#[must_use]
pub fn notification_body(core: &AppCore, event: &AutoEvent, lang: Lang) -> Option<String> {
    let (name, on) = event_subject(core, event)?;
    let key = if on {
        "profile.notice.activated"
    } else {
        "profile.notice.returned"
    };
    Some(crate::phrase::write(
        &Phrase::new(key).text("name", name),
        lang,
    ))
}

/// Looks at the running apps from the timer and tells the user about what it did.
pub(crate) fn tick<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    use tauri::Manager as _;
    use tauri_plugin_notification::NotificationExt as _;

    let state = app.state::<AppState>();
    let Ok(core) = state.core() else {
        return;
    };
    let events = match poll_of(&core) {
        Ok(events) => events,
        Err(error) => {
            log::warn!("profiles: a look at the running apps failed: {error:?}");
            return;
        }
    };
    if events.is_empty() {
        return;
    }
    let lang = language::current(&state);
    let notify = core.automation().is_ok_and(|a| a.config().notify);
    for event in &events {
        if let Some(line) = tooltip_line(&core, event, lang) {
            crate::tray::set_tooltip(app, line);
        }
        if notify
            && app.get_webview_window(crate::window::MAIN_WINDOW).is_none()
            && let Some(body) = notification_body(&core, event, lang)
        {
            // A notification that cannot be shown (disabled in Windows) leaves the tooltip.
            let _ = app.notification().builder().title("Volt").body(body).show();
        }
    }
    if let Ok(view) = list_profiles_of(&core) {
        // A failed emit only means nobody is listening.
        let _ = ProfilesUpdated(view).emit(app);
    }
}

// ---- commands -------------------------------------------------------------------------------------

/// Tauri command: the profiles, which one is active and what the last switch did.
#[tauri::command]
#[specta::specta]
pub(crate) async fn list_profiles(
    state: tauri::State<'_, AppState>,
) -> Result<ProfilesView, IpcError> {
    blocking(&state, list_profiles_of).await
}

/// Tauri command: saves a profile (an empty id makes a new one) and returns the list. A tweak that
/// needs administrator rights, an unknown tweak or a bad app name is refused with `invalidInput`.
#[tauri::command]
#[specta::specta]
pub(crate) async fn save_profile(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    profile: ProfileView,
) -> Result<ProfilesView, IpcError> {
    let view = blocking(&state, move |core| save_profile_of(core, &profile)).await?;
    crate::agent::wake(&app);
    Ok(view)
}

/// Tauri command: removes a profile. What it applied stays applied.
#[tauri::command]
#[specta::specta]
pub(crate) async fn delete_profile(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<ProfilesView, IpcError> {
    let view = blocking(&state, move |core| delete_profile_of(core, &id)).await?;
    crate::agent::wake(&app);
    Ok(view)
}

/// Tauri command: what switching to a profile (or, with no id, turning the active one off) would
/// change right now. Reads the system and the journal only.
#[tauri::command]
#[specta::specta]
pub(crate) async fn preview_profile_switch(
    state: tauri::State<'_, AppState>,
    id: Option<String>,
) -> Result<SwitchPreviewView, IpcError> {
    blocking(&state, move |core| preview_switch_of(core, id.as_deref())).await
}

/// Tauri command: switches profile. Each change goes through the undo journal.
#[tauri::command]
#[specta::specta]
pub(crate) async fn switch_profile(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    id: Option<String>,
) -> Result<SwitchResultView, IpcError> {
    let result = blocking(&state, move |core| switch_of(core, id.as_deref())).await?;
    crate::agent::wake(&app);
    Ok(result)
}

/// Tauri command: puts the last profile switch back as one action.
#[tauri::command]
#[specta::specta]
pub(crate) async fn undo_profile_switch(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<SwitchResultView, IpcError> {
    let result = blocking(&state, undo_switch_of).await?;
    crate::agent::wake(&app);
    Ok(result)
}
