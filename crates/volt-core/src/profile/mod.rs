//! Profiles: named sets of tweak states and automation settings that are switched as one.
//!
//! A [`Profile`] says which tweaks should be on (or off) and, optionally, how the memory rule and
//! the cleaning schedule of [automation](crate::automation) are set. Only tweaks that need no
//! administrator rights can be in a profile. Nothing is active until the user chooses a profile,
//! and the built-in ones ([`builtin_profiles`]) are suggestions that are stored and edited like
//! any other.
//!
//! **Switching** ([`Profiles::plan`], [`Profiles::switch`]) changes only the difference: the
//! tweaks of the profile that was active and are not wanted by the new one are reverted from the
//! journal (only when Volt applied them and they are still in place), and the tweaks the new one
//! wants and are not in place are applied. Each tweak goes through the [`TweakEngine`], so each is
//! its own journal batch that is written before it is changed and undone on its own from the Undo
//! page. The plan is the preview: it names every change and its risk, so Caution and Advanced
//! tweaks are visible before anything happens. A switch stops at the first tweak that fails and
//! says what was changed and what was not; the engine has already rolled back that one tweak, the
//! active profile stays what it was, and switching again finishes the rest (what is already done
//! counts as in place). [`Profiles::undo_switch`] puts the last switch back as one action.
//!
//! **Automatic switches** ([`Profiles::poll`]): a profile may name an app. When that app starts,
//! the profile is activated; when it exits, the profile that was active before returns (if the
//! profile says so and nobody switched in between). The caller hands over the process list; this
//! module starts no thread. An app counts as started when it is seen running after a look at the
//! list that did not see it, so an app that was already running when Volt started never triggers,
//! and one start gives at most one activation and one return. An automatic switch respects the
//! same pauses as the rest of automation (fullscreen, presentation, battery saver, a recent
//! manual clean): a paused switch is skipped for that start, never queued. Automatic switches are
//! journaled with [`Source::Automation`].

use std::collections::HashSet;
use std::sync::{Arc, Mutex, PoisonError};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::activity::{ActivityLog, ActivityOutcome, NewActivity, error_class};
use crate::automation::{Automation, AutomationConfig, MemoryRule, PauseReason, Schedule};
use crate::error::PortError;
use crate::journal::Source;
use crate::label::Risk;
use crate::phrase::Phrase;
use crate::plan::Preview;
use crate::ports::Processes;
use crate::settings::{Settings, SettingsExt};
use crate::time::{Clock, Timestamp};
use crate::tweak::{
    ApplyOutcome, ConfigTweak, ImportAction, ImportSkip, TweakApplyReport, TweakEngine, TweakError,
    TweakRevertReport, TweakState,
};

#[cfg(test)]
mod tests;

/// The settings key that holds the list of profiles.
pub const LIST_KEY: &str = "profiles.list";

/// The settings key that holds the [`ProfileState`].
pub const STATE_KEY: &str = "profiles.state";

/// The label of the activity records a switch leaves (one per switch, next to the records of the
/// tweaks it changed).
pub const SWITCH_LABEL: &str = "profile.switch";

/// The most profiles the list keeps.
pub const MAX_PROFILES: usize = 24;

/// The most tweaks one profile names.
pub const MAX_PROFILE_TWEAKS: usize = 200;

/// The longest profile name, in characters.
pub const MAX_NAME_CHARS: usize = 40;

/// The longest id a profile may have.
pub const MAX_ID_CHARS: usize = 32;

/// The longest executable name a trigger may hold.
pub const MAX_EXE_CHARS: usize = 64;

/// Seconds between two looks at the running apps while a profile has a trigger.
pub const TRIGGER_POLL_SECS: u64 = 15;

fn yes() -> bool {
    true
}

/// One tweak of a profile and the state the profile wants it in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileTweak {
    /// The tweak id.
    pub id: String,
    /// `true`: the tweak is applied. `false`: it is turned off (reverted) when Volt applied it.
    #[serde(default = "yes")]
    pub enabled: bool,
}

/// The app that activates a profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppTrigger {
    /// Executable name, such as `game.exe`. Compared without regard to case.
    pub exe: String,
    /// Whether the profile that was active before returns when the app exits.
    #[serde(default)]
    pub return_on_exit: bool,
}

/// The automation settings a profile sets. A part left `None` is not touched.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProfileAutomation {
    /// The memory rule.
    pub memory: Option<MemoryRule>,
    /// The cleaning schedule.
    pub cleaning: Option<Schedule>,
}

impl ProfileAutomation {
    /// `config` with this profile's parts in place of its own. Notifications stay as they are.
    #[must_use]
    pub fn apply_to(&self, config: &AutomationConfig) -> AutomationConfig {
        let mut next = config.clone();
        if let Some(memory) = self.memory {
            next.memory = memory;
        }
        if let Some(cleaning) = &self.cleaning {
            next.cleaning = cleaning.clone();
        }
        next
    }

    fn as_config(&self) -> AutomationConfig {
        self.apply_to(&AutomationConfig::default())
    }
}

/// A named set of tweak states and automation settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    /// Stable id: lower-case letters, digits and `-`. Empty for a profile not saved yet.
    pub id: String,
    /// The name shown to the user.
    pub name: String,
    /// The tweaks, each once, in the order they are applied.
    pub tweaks: Vec<ProfileTweak>,
    /// The automation settings it sets.
    pub automation: ProfileAutomation,
    /// The app that activates it, if any.
    pub trigger: Option<AppTrigger>,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.chars().count() <= MAX_ID_CHARS
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// An executable name as typed: a plain file name ending in `.exe`, with no folder.
fn valid_exe(exe: &str) -> bool {
    let lower = exe.to_lowercase();
    lower.chars().count() <= MAX_EXE_CHARS
        && lower.len() > ".exe".len()
        && std::path::Path::new(&lower)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
        && !lower.chars().any(|c| {
            matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control()
        })
        && lower.trim() == lower
}

/// Why a profile or a switch could not be made. Nothing was changed, unless the switch report
/// says otherwise.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum ProfileError {
    /// There is no profile with this id.
    #[error("there is no profile `{0}`")]
    Unknown(String),
    /// The name is empty or too long.
    #[error("a profile needs a name of 1 to {MAX_NAME_CHARS} characters")]
    Name,
    /// The list is full.
    #[error("at most {MAX_PROFILES} profiles are kept")]
    TooMany,
    /// The profile names too many tweaks.
    #[error("a profile holds at most {MAX_PROFILE_TWEAKS} tweaks")]
    TooManyTweaks,
    /// The profile names a tweak this build does not have.
    #[error("unknown tweak `{0}`")]
    UnknownTweak(String),
    /// The profile names a tweak that needs administrator rights.
    #[error("tweak `{0}` needs administrator rights, so a profile cannot include it")]
    NeedsAdministrator(String),
    /// A tweak is listed twice.
    #[error("tweak `{0}` is listed twice")]
    DuplicateTweak(String),
    /// The app to watch is not an executable name.
    #[error("the app to watch must be an executable name such as `game.exe`")]
    Trigger,
    /// A number of the automation settings is out of range.
    #[error("the automation settings of the profile are out of range")]
    Automation,
    /// No switch was made, or it was already undone.
    #[error("there is no profile switch to undo")]
    NothingToUndo,
    /// The settings could not be read or written.
    #[error("the profiles could not be stored: {0}")]
    Store(PortError),
}

impl ProfileError {
    /// The error as a code and parameters, for the window to write in its own language.
    #[must_use]
    pub fn phrase(&self) -> Phrase {
        match self {
            Self::Unknown(_) => Phrase::new("err.profile.unknown"),
            Self::Name => Phrase::new("err.profile.name").count("max", MAX_NAME_CHARS),
            Self::TooMany => Phrase::new("err.profile.tooMany").count("max", MAX_PROFILES),
            Self::TooManyTweaks => {
                Phrase::new("err.profile.tooManyTweaks").count("max", MAX_PROFILE_TWEAKS)
            }
            Self::UnknownTweak(id) => Phrase::new("err.profile.tweakUnknown").text("id", id),
            Self::NeedsAdministrator(id) => Phrase::new("err.profile.tweakAdmin").text("id", id),
            Self::DuplicateTweak(id) => Phrase::new("err.profile.tweakDuplicate").text("id", id),
            Self::Trigger => Phrase::new("err.profile.trigger"),
            Self::Automation => Phrase::new("err.profile.automation"),
            Self::NothingToUndo => Phrase::new("err.profile.nothingToUndo"),
            Self::Store(_) => Phrase::new("err.profile.store"),
        }
    }
}

impl From<PortError> for ProfileError {
    fn from(error: PortError) -> Self {
        Self::Store(error)
    }
}

/// Which profile is active and what the last switch did. Everything else is derived.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProfileState {
    /// The profile whose switch was completed last (none until the user chooses one).
    pub active: Option<String>,
    /// The last switch, for [`Profiles::undo_switch`].
    pub last_switch: Option<SwitchRecord>,
    /// Set by an automatic activation: the profile to return to when the app exits.
    pub auto_return: Option<AutoReturn>,
    /// The last automatic switch that a pause skipped.
    pub last_skip: Option<AutoSkip>,
}

/// What a switch changed, kept so it can be undone as one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwitchRecord {
    /// When.
    pub at: Timestamp,
    /// Who started it.
    pub source: Source,
    /// The active profile before.
    pub from: Option<String>,
    /// The profile it switched to.
    pub to: Option<String>,
    /// The tweaks it applied.
    pub applied: Vec<String>,
    /// The tweaks it reverted.
    pub reverted: Vec<String>,
    /// The automation settings before, when the switch changed them.
    pub automation_before: Option<AutomationConfig>,
    /// Whether it finished. A switch that stopped part-way keeps its record so what it did can be
    /// undone.
    pub complete: bool,
}

/// The profile an automatic activation returns to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutoReturn {
    /// The profile the app activated.
    pub profile: String,
    /// The profile that was active before it (none when there was none).
    pub previous: Option<String>,
}

/// An automatic switch that a pause skipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutoSkip {
    /// When.
    pub at: Timestamp,
    /// Why.
    pub reason: PauseReason,
}

/// The profiles of a person, their state and the switching between them.
pub struct Profiles {
    settings: Arc<dyn Settings>,
    activity: Arc<dyn ActivityLog>,
    clock: Arc<dyn Clock>,
    engine: TweakEngine,
    processes: Arc<dyn Processes>,
    /// The watched apps seen running at the last look; `None` before the first look.
    seen: Mutex<Option<HashSet<String>>>,
}

impl std::fmt::Debug for Profiles {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Profiles").finish_non_exhaustive()
    }
}

/// What a switch would do to one tweak.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SwitchAction {
    /// Apply it: the changes it makes now.
    TurnOn(Vec<Preview>),
    /// Revert what Volt applied, from the journal.
    TurnOff,
}

/// One tweak a switch changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitchItem {
    /// The tweak id.
    pub tweak: String,
    /// Its name.
    pub title: String,
    /// How dangerous it is; Caution and Advanced tweaks are named in the preview.
    pub risk: Risk,
    /// What happens.
    pub action: SwitchAction,
}

/// Why a tweak of a profile is not changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeftOut {
    /// This build has no such tweak.
    Unknown,
    /// It needs administrator rights, which a profile does not use.
    NeedsAdministrator,
    /// It cannot be applied here, or its state cannot be read. Holds the reason.
    Unavailable(String),
    /// The profile wants it off but it is on because Windows or the user set it: Volt has no
    /// earlier value to put back.
    NotAppliedByVolt,
}

/// A tweak of a profile that a switch leaves alone, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotIncluded {
    /// The tweak id.
    pub tweak: String,
    /// Its name, when this build knows it.
    pub title: Option<String>,
    /// Why.
    pub reason: LeftOut,
}

/// The automation settings before and after a switch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutomationChange {
    /// What is saved now.
    pub before: AutomationConfig,
    /// What the switch saves.
    pub after: AutomationConfig,
}

/// Exactly what a switch would change right now. Reading it changes nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitchPlan {
    /// The active profile now.
    pub from: Option<String>,
    /// The profile to switch to; `None` turns the active profile off.
    pub to: Option<String>,
    /// The tweaks to change: turned off first, then on.
    pub items: Vec<SwitchItem>,
    /// Tweaks the switch leaves alone, with the reason.
    pub left_out: Vec<NotIncluded>,
    /// How many tweaks the profile wants that are in place already.
    pub in_place: usize,
    /// The automation change, when there is one.
    pub automation: Option<AutomationChange>,
}

impl SwitchPlan {
    /// Whether the switch changes nothing (only which profile is active).
    #[must_use]
    pub fn is_noop(&self) -> bool {
        self.items.is_empty() && self.automation.is_none()
    }

    /// The tweaks that are Caution or Advanced.
    pub fn risky(&self) -> impl Iterator<Item = &SwitchItem> {
        self.items.iter().filter(|item| item.risk != Risk::Safe)
    }
}

/// How one tweak of a switch ended.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum StepOutcome {
    /// Changed.
    Done,
    /// Nothing was left to change (it had already been done, or was not applied by Volt).
    Nothing,
    /// The apply failed; the report says whether it was rolled back.
    ApplyFailed(TweakApplyReport),
    /// The revert did not restore everything.
    RevertIncomplete(TweakRevertReport),
    /// The tweak could not be read or changed at all.
    Failed(TweakError),
    /// The switch stopped before it got here.
    NotAttempted,
}

impl StepOutcome {
    fn stops(&self) -> bool {
        matches!(
            self,
            Self::ApplyFailed(_) | Self::RevertIncomplete(_) | Self::Failed(_)
        )
    }
}

/// One tweak of a switch and how it ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitchStep {
    /// The tweak id.
    pub tweak: String,
    /// Its name.
    pub title: String,
    /// `true` for an apply, `false` for a revert.
    pub turned_on: bool,
    /// How it ended.
    pub outcome: StepOutcome,
}

/// How the automation part of a switch ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutomationOutcome {
    /// The profile sets none, or it was already so.
    NotChanged,
    /// Saved.
    Set,
    /// Could not be saved.
    Failed(PortError),
    /// The switch stopped before it got here.
    NotAttempted,
}

/// What a switch (or its undo) did, precisely.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitchReport {
    /// The active profile before.
    pub from: Option<String>,
    /// The profile the switch aimed at.
    pub to: Option<String>,
    /// Every tweak, in the order it was handled.
    pub steps: Vec<SwitchStep>,
    /// The automation settings.
    pub automation: AutomationOutcome,
    /// Whether everything was done. When it is not, the active profile is unchanged and what was
    /// done is in [`ProfileState::last_switch`].
    pub complete: bool,
}

impl SwitchReport {
    /// How many tweaks the switch changed.
    #[must_use]
    pub fn changed(&self) -> usize {
        self.steps
            .iter()
            .filter(|step| step.outcome == StepOutcome::Done)
            .count()
    }
}

/// What one look at the running apps did.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AutoEvent {
    /// An app started and its profile was activated (or tried to be).
    Activated(SwitchReport),
    /// An app exited and the earlier profile came back (or tried to).
    Returned(SwitchReport),
    /// A pause was on, so the switch was skipped for this start (or exit).
    Skipped {
        /// The profile that was meant to be active (or left).
        profile: String,
        /// Why.
        reason: PauseReason,
    },
}

impl Profiles {
    /// Profiles over the given ports and tweak engine; `processes` is read by [`Profiles::poll`]
    /// only. Nothing is active.
    #[must_use]
    pub fn new(
        settings: Arc<dyn Settings>,
        activity: Arc<dyn ActivityLog>,
        clock: Arc<dyn Clock>,
        engine: TweakEngine,
        processes: Arc<dyn Processes>,
    ) -> Self {
        Self {
            settings,
            activity,
            clock,
            engine,
            processes,
            seen: Mutex::new(None),
        }
    }

    // ---- the list ----------------------------------------------------------------------------

    /// The list the user saved, or `None` while they never saved one.
    #[must_use]
    pub fn saved(&self) -> Option<Vec<Profile>> {
        match self.settings.get_typed::<Vec<Profile>>(LIST_KEY) {
            Ok(saved) => saved,
            Err(error) => {
                log::warn!(
                    "the profiles could not be read ({}); the list starts again",
                    error_class(&error)
                );
                Some(Vec::new())
            }
        }
    }

    /// The profiles: what the user saved, or the built-in suggestions until they save.
    #[must_use]
    pub fn list(&self) -> Vec<Profile> {
        self.saved().unwrap_or_else(builtin_profiles)
    }

    fn store_list(&self, list: &[Profile]) -> Result<(), ProfileError> {
        Ok(self.settings.set_typed(LIST_KEY, &list)?)
    }

    /// What is active and what the last switch did.
    #[must_use]
    pub fn state(&self) -> ProfileState {
        self.settings
            .get_typed::<ProfileState>(STATE_KEY)
            .unwrap_or_else(|error| {
                log::warn!(
                    "the profile state could not be read ({}); no profile is active",
                    error_class(&error)
                );
                None
            })
            .unwrap_or_default()
    }

    fn store_state(&self, state: &ProfileState) {
        if let Err(error) = self.settings.set_typed(STATE_KEY, state) {
            log::warn!(
                "the profile state could not be saved ({})",
                error_class(&error)
            );
        }
    }

    /// Checks a profile the user entered. An id that is empty is allowed (it is chosen on save).
    pub fn validate(&self, profile: &Profile) -> Result<(), ProfileError> {
        let name = profile.name.trim();
        if name.is_empty() || name.chars().count() > MAX_NAME_CHARS {
            return Err(ProfileError::Name);
        }
        if !profile.id.is_empty() && !valid_id(&profile.id) {
            return Err(ProfileError::Unknown(profile.id.clone()));
        }
        if profile.tweaks.len() > MAX_PROFILE_TWEAKS {
            return Err(ProfileError::TooManyTweaks);
        }
        let mut seen = HashSet::new();
        for entry in &profile.tweaks {
            let Some(tweak) = self.engine.catalogue().get(&entry.id) else {
                return Err(ProfileError::UnknownTweak(entry.id.clone()));
            };
            if tweak.requires_admin {
                return Err(ProfileError::NeedsAdministrator(entry.id.clone()));
            }
            if !seen.insert(entry.id.as_str()) {
                return Err(ProfileError::DuplicateTweak(entry.id.clone()));
            }
        }
        if let Some(trigger) = &profile.trigger
            && !valid_exe(&trigger.exe)
        {
            return Err(ProfileError::Trigger);
        }
        profile
            .automation
            .as_config()
            .validate()
            .map_err(|_| ProfileError::Automation)
    }

    /// Saves a profile: a new one (empty id, which gets one) or a replacement of the profile with
    /// the same id. Returns the saved profile.
    pub fn save(&self, mut profile: Profile) -> Result<Profile, ProfileError> {
        let trimmed = profile.name.trim().to_owned();
        profile.name = trimmed;
        self.validate(&profile)?;
        if let Some(trigger) = &mut profile.trigger {
            trigger.exe = trigger.exe.to_lowercase();
        }
        let mut list = self.list();
        if profile.id.is_empty() {
            profile.id = (1..=MAX_PROFILES + 1)
                .map(|n| format!("profile-{n}"))
                .find(|id| list.iter().all(|other| &other.id != id))
                .unwrap_or_default();
        }
        match list.iter().position(|other| other.id == profile.id) {
            Some(at) => list[at] = profile.clone(),
            None if list.len() >= MAX_PROFILES => return Err(ProfileError::TooMany),
            None => list.push(profile.clone()),
        }
        self.store_list(&list)?;
        Ok(profile)
    }

    /// Removes a profile. What it applied stays applied (undo it from the Undo page); if it was the
    /// active profile, none is active afterwards. Returns whether it existed.
    pub fn delete(&self, id: &str) -> Result<bool, ProfileError> {
        let mut list = self.list();
        let before = list.len();
        list.retain(|profile| profile.id != id);
        if list.len() == before {
            return Ok(false);
        }
        self.store_list(&list)?;
        let mut state = self.state();
        if state.active.as_deref() == Some(id) {
            state.active = None;
        }
        if state.auto_return.as_ref().is_some_and(|r| r.profile == id) {
            state.auto_return = None;
        }
        self.store_state(&state);
        Ok(true)
    }

    /// A profile of a file made safe to keep: only tweaks this build knows and that need no
    /// administrator rights, a plain executable name, numbers within their bounds. `None` for a
    /// profile with an unusable id or name. Also returns how many tweaks (or triggers) were left
    /// out.
    #[must_use]
    pub fn sanitized(&self, profile: Profile) -> Option<(Profile, usize)> {
        let name: String = profile.name.trim().chars().take(MAX_NAME_CHARS).collect();
        if !valid_id(&profile.id) || name.is_empty() {
            return None;
        }
        let mut dropped = 0;
        let mut seen = HashSet::new();
        let mut tweaks = Vec::new();
        for entry in profile.tweaks.into_iter().take(MAX_PROFILE_TWEAKS) {
            let known = self
                .engine
                .catalogue()
                .get(&entry.id)
                .is_some_and(|tweak| !tweak.requires_admin);
            if known && seen.insert(entry.id.clone()) {
                tweaks.push(entry);
            } else {
                dropped += 1;
            }
        }
        let trigger = match profile.trigger {
            Some(trigger) if valid_exe(&trigger.exe) => Some(AppTrigger {
                exe: trigger.exe.to_lowercase(),
                ..trigger
            }),
            Some(_) => {
                dropped += 1;
                None
            }
            None => None,
        };
        let clamped = profile.automation.as_config().clamped();
        let automation = ProfileAutomation {
            memory: profile.automation.memory.map(|_| clamped.memory),
            cleaning: profile.automation.cleaning.map(|_| clamped.cleaning),
        };
        Some((
            Profile {
                id: profile.id,
                name,
                tweaks,
                automation,
                trigger,
            },
            dropped,
        ))
    }

    // ---- planning and switching --------------------------------------------------------------

    fn title_of(&self, id: &str) -> String {
        self.engine
            .catalogue()
            .get(id)
            .map_or_else(|| id.to_owned(), |tweak| tweak.title.clone())
    }

    /// What a switch to `target` (`None`: turn the active profile off) would change right now.
    /// Read-only.
    pub fn plan(
        &self,
        target: Option<&str>,
        automation: &Automation,
    ) -> Result<SwitchPlan, ProfileError> {
        let list = self.list();
        let to = match target {
            Some(id) => Some(
                list.iter()
                    .find(|profile| profile.id == id)
                    .ok_or_else(|| ProfileError::Unknown(id.to_owned()))?,
            ),
            None => None,
        };
        let state = self.state();
        let from = state
            .active
            .as_deref()
            .and_then(|id| list.iter().find(|profile| profile.id == id));

        let (mut items, mut left_out) = self.turn_offs(from, to);
        let in_place = self.turn_ons(to, &mut items, &mut left_out);
        let automation_change = to.and_then(|to| {
            let before = automation.config();
            let after = to.automation.apply_to(&before);
            (after != before).then_some(AutomationChange { before, after })
        });
        Ok(SwitchPlan {
            from: from.map(|profile| profile.id.clone()),
            to: to.map(|profile| profile.id.clone()),
            items,
            left_out,
            in_place,
            automation: automation_change,
        })
    }

    /// The tweaks a switch turns off: those the active profile turned on that the new one does not
    /// want on, and those the new one says must be off. Only what Volt applied and is still in
    /// place can be put back.
    fn turn_offs(
        &self,
        from: Option<&Profile>,
        to: Option<&Profile>,
    ) -> (Vec<SwitchItem>, Vec<NotIncluded>) {
        let mut off: Vec<&str> = Vec::new();
        if let Some(from) = from {
            for entry in from.tweaks.iter().filter(|entry| entry.enabled) {
                let kept = to.is_some_and(|to| to.tweaks.iter().any(|t| t.id == entry.id));
                if !kept {
                    off.push(&entry.id);
                }
            }
        }
        if let Some(to) = to {
            for entry in to.tweaks.iter().filter(|entry| !entry.enabled) {
                if !off.contains(&entry.id.as_str()) {
                    off.push(&entry.id);
                }
            }
        }
        let mut items = Vec::new();
        let mut left_out = Vec::new();
        for id in off {
            let Some(tweak) = self.engine.catalogue().get(id) else {
                continue;
            };
            match self.engine.state(id) {
                Ok(TweakState::Applied) => {
                    if self.engine.applied_by_volt(id).unwrap_or(false) {
                        items.push(SwitchItem {
                            tweak: id.to_owned(),
                            title: tweak.title.clone(),
                            risk: tweak.risk,
                            action: SwitchAction::TurnOff,
                        });
                    } else {
                        left_out.push(NotIncluded {
                            tweak: id.to_owned(),
                            title: Some(tweak.title.clone()),
                            reason: LeftOut::NotAppliedByVolt,
                        });
                    }
                }
                // Already off (or changed behind Volt's back): nothing to put back.
                Ok(_) => {}
                Err(error) => left_out.push(NotIncluded {
                    tweak: id.to_owned(),
                    title: Some(tweak.title.clone()),
                    reason: LeftOut::Unavailable(error.to_string()),
                }),
            }
        }
        (items, left_out)
    }

    /// Adds the tweaks the new profile wants on and that are not in place to `items`, and those it
    /// cannot have to `left_out`. Returns how many are in place already.
    fn turn_ons(
        &self,
        to: Option<&Profile>,
        items: &mut Vec<SwitchItem>,
        left_out: &mut Vec<NotIncluded>,
    ) -> usize {
        let mut in_place = 0;
        let Some(to) = to else {
            return in_place;
        };
        for entry in to.tweaks.iter().filter(|entry| entry.enabled) {
            let wanted = [ConfigTweak {
                id: entry.id.clone(),
                title: None,
            }];
            let known = self.engine.catalogue().get(&entry.id);
            for planned in self.engine.plan_import(&wanted) {
                match planned.action {
                    ImportAction::Apply(previews) => items.push(SwitchItem {
                        tweak: entry.id.clone(),
                        title: known.map_or_else(|| entry.id.clone(), |t| t.title.clone()),
                        risk: known.map_or(Risk::Advanced, |t| t.risk),
                        action: SwitchAction::TurnOn(previews),
                    }),
                    ImportAction::AlreadyApplied => in_place += 1,
                    ImportAction::Skip(why) => left_out.push(NotIncluded {
                        tweak: entry.id.clone(),
                        title: known.map(|t| t.title.clone()),
                        reason: match why {
                            ImportSkip::NeedsAdministrator => LeftOut::NeedsAdministrator,
                            ImportSkip::Unavailable(reason) => LeftOut::Unavailable(reason),
                            _ => LeftOut::Unknown,
                        },
                    }),
                }
            }
        }
        in_place
    }

    fn run_action(&self, tweak: &str, turn_on: bool, source: Source) -> StepOutcome {
        if turn_on {
            match self.engine.apply(tweak, source) {
                Ok(report) => match report.outcome {
                    ApplyOutcome::Applied => StepOutcome::Done,
                    ApplyOutcome::AlreadyApplied => StepOutcome::Nothing,
                    _ => StepOutcome::ApplyFailed(report),
                },
                Err(error) => StepOutcome::Failed(error),
            }
        } else {
            match self.engine.revert_as(tweak, source) {
                Ok(report) if report.is_complete() => StepOutcome::Done,
                Ok(report) => StepOutcome::RevertIncomplete(report),
                Err(TweakError::NothingToRevert(_)) => StepOutcome::Nothing,
                Err(error) => StepOutcome::Failed(error),
            }
        }
    }

    /// Runs `(tweak, turn_on)` pairs in order and stops at the first that fails. The tweaks after
    /// it are reported as not attempted.
    fn run_steps(&self, work: &[(String, bool)], source: Source) -> Vec<SwitchStep> {
        let mut stopped = false;
        work.iter()
            .map(|(tweak, turn_on)| {
                let outcome = if stopped {
                    StepOutcome::NotAttempted
                } else {
                    self.run_action(tweak, *turn_on, source)
                };
                stopped |= outcome.stops();
                SwitchStep {
                    title: self.title_of(tweak),
                    tweak: tweak.clone(),
                    turned_on: *turn_on,
                    outcome,
                }
            })
            .collect()
    }

    fn say(&self, source: Source, outcome: ActivityOutcome, phrase: Phrase) {
        let activity = NewActivity::said(
            self.clock.now(),
            source,
            outcome,
            phrase,
            Some(SWITCH_LABEL.to_owned()),
        );
        if let Err(error) = self.activity.record(activity) {
            log::warn!(
                "a profile switch could not be recorded ({})",
                error_class(&error)
            );
        }
    }

    fn name_of(&self, id: Option<&str>) -> String {
        id.and_then(|id| self.list().into_iter().find(|profile| profile.id == id))
            .map(|profile| profile.name)
            .unwrap_or_default()
    }

    /// Switches to `target` (`None` turns the active profile off): reverts and applies only the
    /// difference, tweak by tweak through the engine, then sets the automation settings. Stops at
    /// the first failure. Switching to the profile that is active changes nothing. The caller
    /// holds whatever lock keeps other changes out.
    pub fn switch(
        &self,
        target: Option<&str>,
        source: Source,
        automation: &Automation,
    ) -> Result<SwitchReport, ProfileError> {
        let plan = self.plan(target, automation)?;
        let mut state = self.state();
        state.auto_return = None;
        if plan.from == plan.to && plan.is_noop() {
            self.store_state(&state);
            return Ok(SwitchReport {
                from: plan.from,
                to: plan.to,
                steps: Vec::new(),
                automation: AutomationOutcome::NotChanged,
                complete: true,
            });
        }
        let work: Vec<(String, bool)> = plan
            .items
            .iter()
            .map(|item| {
                (
                    item.tweak.clone(),
                    matches!(item.action, SwitchAction::TurnOn(_)),
                )
            })
            .collect();
        let steps = self.run_steps(&work, source);
        let mut stopped = steps.iter().any(|step| step.outcome.stops());

        let mut automation_before = None;
        let automation_outcome = match &plan.automation {
            None => AutomationOutcome::NotChanged,
            Some(_) if stopped => AutomationOutcome::NotAttempted,
            Some(change) => match automation.save_config(&change.after) {
                Ok(()) => {
                    automation_before = Some(change.before.clone());
                    AutomationOutcome::Set
                }
                Err(error) => {
                    stopped = true;
                    AutomationOutcome::Failed(error)
                }
            },
        };

        let done = |turned_on: bool| -> Vec<String> {
            steps
                .iter()
                .filter(|step| step.turned_on == turned_on && step.outcome == StepOutcome::Done)
                .map(|step| step.tweak.clone())
                .collect()
        };
        let (applied, reverted) = (done(true), done(false));
        let complete = !stopped;
        if complete || !applied.is_empty() || !reverted.is_empty() || automation_before.is_some() {
            state.last_switch = Some(SwitchRecord {
                at: self.clock.now(),
                source,
                from: plan.from.clone(),
                to: plan.to.clone(),
                applied,
                reverted,
                automation_before,
                complete,
            });
        }
        if complete {
            state.active.clone_from(&plan.to);
        }
        self.store_state(&state);

        let name = self.name_of(plan.to.as_deref().or(plan.from.as_deref()));
        let phrase = match (complete, plan.to.is_some()) {
            (true, true) => Phrase::new("profile.switched").text("name", name),
            (true, false) => Phrase::new("profile.deactivated").text("name", name),
            (false, _) => Phrase::new("profile.switch.stopped").text("name", name),
        };
        self.say(
            source,
            if complete {
                ActivityOutcome::Applied
            } else {
                ActivityOutcome::Failed
            },
            phrase,
        );
        Ok(SwitchReport {
            from: plan.from,
            to: plan.to,
            steps,
            automation: automation_outcome,
            complete,
        })
    }

    /// Puts the last switch back as one action: what it applied is reverted, what it reverted is
    /// applied again, and the automation settings and the active profile are what they were. A
    /// tweak that is not in the state the switch left it in is skipped (there is nothing to put
    /// back). Stops at the first failure and keeps what is left of the record.
    pub fn undo_switch(&self, automation: &Automation) -> Result<SwitchReport, ProfileError> {
        let mut state = self.state();
        let record = state
            .last_switch
            .clone()
            .ok_or(ProfileError::NothingToUndo)?;
        let mut work: Vec<(String, bool)> = Vec::new();
        for id in record.applied.iter().rev() {
            if self.engine.state(id) == Ok(TweakState::Applied) {
                work.push((id.clone(), false));
            }
        }
        for id in &record.reverted {
            if self.engine.state(id) != Ok(TweakState::Applied) {
                work.push((id.clone(), true));
            }
        }
        let steps = self.run_steps(&work, Source::Manual);
        let mut stopped = steps.iter().any(|step| step.outcome.stops());
        let automation_outcome = match &record.automation_before {
            None => AutomationOutcome::NotChanged,
            Some(_) if stopped => AutomationOutcome::NotAttempted,
            Some(before) => match automation.save_config(before) {
                Ok(()) => AutomationOutcome::Set,
                Err(error) => {
                    stopped = true;
                    AutomationOutcome::Failed(error)
                }
            },
        };
        let complete = !stopped;
        if complete {
            state.active.clone_from(&record.from);
            state.last_switch = None;
            state.auto_return = None;
        } else {
            // What is done is no longer part of the record.
            let finished = |id: &String| {
                steps
                    .iter()
                    .any(|s| &s.tweak == id && s.outcome == StepOutcome::Done)
            };
            let mut rest = record.clone();
            rest.applied.retain(|id| !finished(id));
            rest.reverted.retain(|id| !finished(id));
            if matches!(automation_outcome, AutomationOutcome::Set) {
                rest.automation_before = None;
            }
            state.last_switch = Some(rest);
        }
        self.store_state(&state);
        let name = self.name_of(record.to.as_deref().or(record.from.as_deref()));
        self.say(
            Source::Manual,
            if complete {
                ActivityOutcome::Reverted
            } else {
                ActivityOutcome::Failed
            },
            Phrase::new(if complete {
                "profile.switch.undone"
            } else {
                "profile.switch.undoStopped"
            })
            .text("name", name),
        );
        Ok(SwitchReport {
            from: record.to,
            to: record.from,
            steps,
            automation: automation_outcome,
            complete,
        })
    }

    // ---- automatic switches ------------------------------------------------------------------

    /// Whether a look at the running apps can matter: a profile names an app, or an automatic
    /// activation waits for its app to exit.
    #[must_use]
    pub fn watching(&self) -> bool {
        self.list().iter().any(|profile| profile.trigger.is_some())
            || self.state().auto_return.is_some()
    }

    fn seen_guard(&self) -> std::sync::MutexGuard<'_, Option<HashSet<String>>> {
        self.seen.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// One look at the running apps: activates the profile of an app that just started and brings
    /// the earlier profile back when that app has exited. Reads the process list once, and only
    /// while [`Profiles::watching`]. `pause` says whether a pause is on (see the module
    /// documentation); the caller holds whatever lock keeps other changes out.
    pub fn poll(
        &self,
        pause: &dyn Fn() -> Option<PauseReason>,
        automation: &Automation,
    ) -> Vec<AutoEvent> {
        let list = self.list();
        let initial = self.state();
        let triggers: Vec<(&Profile, String, bool)> = list
            .iter()
            .filter_map(|profile| {
                profile
                    .trigger
                    .as_ref()
                    .map(|t| (profile, t.exe.to_lowercase(), t.return_on_exit))
            })
            .collect();
        if triggers.is_empty() && initial.auto_return.is_none() {
            *self.seen_guard() = None;
            return Vec::new();
        }
        let running: HashSet<String> = match self.processes.list() {
            Ok(found) => found
                .into_iter()
                .filter(|process| process.session_id != 0)
                .map(|process| process.name.to_lowercase())
                .collect(),
            Err(error) => {
                log::warn!(
                    "profiles: the running apps could not be read ({})",
                    error_class(&error)
                );
                return Vec::new();
            }
        };
        let watched: HashSet<String> = triggers
            .iter()
            .filter(|(_, exe, _)| running.contains(exe))
            .map(|(_, exe, _)| exe.clone())
            .collect();
        let previous = self.seen_guard().replace(watched.clone());

        let mut events = Vec::new();
        if let Some(back) = &initial.auto_return {
            let exe = triggers
                .iter()
                .find(|(profile, _, _)| profile.id == back.profile)
                .map(|(_, exe, _)| exe.as_str());
            self.poll_exit(back, exe, &running, pause, automation, &mut events);
        }
        let Some(previous) = previous else {
            // The first look is only a baseline: apps that are already running did not start now.
            return events;
        };
        let started = triggers
            .iter()
            .find(|(_, exe, _)| watched.contains(exe) && !previous.contains(exe));
        let Some((profile, _, return_on_exit)) = started else {
            return events;
        };
        let state = self.state();
        if state.active.as_deref() == Some(profile.id.as_str()) {
            return events;
        }
        if let Some(reason) = pause() {
            self.skip(reason);
            events.push(AutoEvent::Skipped {
                profile: profile.id.clone(),
                reason,
            });
            return events;
        }
        match self.switch(Some(&profile.id), Source::Automation, automation) {
            Ok(report) => {
                if report.complete && *return_on_exit {
                    let mut state = self.state();
                    state.auto_return = Some(AutoReturn {
                        profile: profile.id.clone(),
                        previous: report.from.clone(),
                    });
                    self.store_state(&state);
                }
                events.push(AutoEvent::Activated(report));
            }
            Err(error) => log::warn!("profiles: an automatic switch failed: {error}"),
        }
        events
    }

    /// The app of an automatic activation may have exited: bring the earlier profile back.
    fn poll_exit(
        &self,
        back: &AutoReturn,
        exe: Option<&str>,
        running: &HashSet<String>,
        pause: &dyn Fn() -> Option<PauseReason>,
        automation: &Automation,
        events: &mut Vec<AutoEvent>,
    ) {
        let still_ours = self.state().active.as_deref() == Some(back.profile.as_str());
        let Some(exe) = exe.filter(|_| still_ours) else {
            // The profile or its trigger is gone, or the user chose another profile: nothing to
            // return from.
            let mut state = self.state();
            state.auto_return = None;
            self.store_state(&state);
            return;
        };
        if running.contains(exe) {
            return;
        }
        if let Some(reason) = pause() {
            self.skip(reason);
            events.push(AutoEvent::Skipped {
                profile: back.profile.clone(),
                reason,
            });
            return;
        }
        let previous = back
            .previous
            .as_deref()
            .filter(|id| self.list().iter().any(|profile| profile.id == *id));
        match self.switch(previous, Source::Automation, automation) {
            Ok(report) => events.push(AutoEvent::Returned(report)),
            Err(error) => log::warn!("profiles: the return to the earlier profile failed: {error}"),
        }
    }

    fn skip(&self, reason: PauseReason) {
        let mut state = self.state();
        state.last_skip = Some(AutoSkip {
            at: self.clock.now(),
            reason,
        });
        self.store_state(&state);
    }
}

/// The profiles suggested to a person who has not saved any: Gaming, Work, AI workload and
/// Battery. They only name tweaks of the catalogue (each explained there as usual) and are edited
/// or deleted like any other profile. None is active until the user chooses one.
#[must_use]
pub fn builtin_profiles() -> Vec<Profile> {
    let tweaks = |ids: &[&str]| {
        ids.iter()
            .map(|id| ProfileTweak {
                id: (*id).to_owned(),
                enabled: true,
            })
            .collect()
    };
    let memory = |standby_above_mb, enabled| {
        Some(MemoryRule {
            enabled,
            standby_above_mb,
            ..MemoryRule::default()
        })
    };
    let profile = |id: &str, name: &str, ids: &[&str], memory: Option<MemoryRule>| Profile {
        id: id.to_owned(),
        name: name.to_owned(),
        tweaks: tweaks(ids),
        automation: ProfileAutomation {
            memory,
            cleaning: None,
        },
        trigger: None,
    };
    vec![
        profile(
            "gaming",
            "Gaming",
            &[
                "turn-off-game-recording",
                "faster-menu-display",
                "turn-off-window-animations",
                "turn-off-transparency",
                "end-hung-apps-faster",
            ],
            memory(4096, true),
        ),
        profile(
            "work",
            "Work",
            &[
                "show-file-extensions",
                "show-seconds-in-clock",
                "turn-off-windows-tips",
                "turn-off-settings-suggestions",
            ],
            None,
        ),
        profile(
            "ai-workload",
            "AI workload",
            &[
                "turn-off-game-recording",
                "turn-off-transparency",
                "turn-off-window-animations",
            ],
            memory(8192, true),
        ),
        profile(
            "battery",
            "Battery",
            &[
                "turn-off-transparency",
                "turn-off-window-animations",
                "turn-off-search-highlights",
                "stop-tracking-app-launches",
            ],
            memory(4096, false),
        ),
    ]
}
