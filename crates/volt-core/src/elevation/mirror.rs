//! The UI's copy of the helper's activity log.
//!
//! The helper keeps its own journal and activity log (the UI never opens them and the helper never
//! opens the UI's). The UI mirrors the log by PULL: it asks for the records after the last one it
//! holds, keyed by (store id, record id), and keeps one cursor per helper store. A store that was
//! recreated has a new id, so its first records are fetched from the start, and the rows of the old
//! one stay visible without Undo.
//!
//! Pulling never starts the helper: [`pull`] does nothing when no session is open, and the UI pulls
//! after each administrator change (which has a session) and whenever the Undo page opens with a
//! session open. What was pulled is saved by the shell (this type is plain data) so the rows stay
//! on the Undo page, read-only, when the helper is not running.
//!
//! The mirror holds rows only. Whether a row can be undone is derived here from the rows
//! themselves ([`HelperMirror::rows`]): the helper's `tweak_revert` undoes everything it applied
//! for a tweak, so a row is undoable when it is an apply that no later revert of the same tweak
//! followed, in the current store.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{ElevationError, Elevator, HISTORY_MAX_RECORDS};
use crate::activity::{ActivityOutcome, ActivityRecord};

/// Rows kept per helper store. The helper's log is small (one row per administrator change); this
/// only bounds a runaway.
pub const MAX_ROWS_PER_STORE: usize = 500;

/// Pages one pull reads at most.
const MAX_PAGES: usize = 100;

/// A record id that no store reaches: asking after it returns the store id and no records.
const PROBE_AFTER: u64 = i64::MAX as u64;

/// What the mirror holds of one helper store.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MirroredStore {
    /// The highest record id pulled, kept even when the rows were cleared so they are not fetched
    /// again.
    pub cursor: u64,
    /// The rows, oldest first.
    pub records: Vec<ActivityRecord>,
}

/// The UI's copy of the helper's activity log.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelperMirror {
    /// The store of the helper that answered last. Rows of any other store cannot be undone.
    pub current: Option<String>,
    /// Every store seen, by its id.
    pub stores: BTreeMap<String, MirroredStore>,
}

/// Where a mirrored row stands with respect to undo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowState {
    /// An apply nothing reverted since: the helper can undo it.
    CanUndo,
    /// An apply that a later revert took back.
    Undone,
    /// An attempt that failed or was refused: nothing to undo.
    Failed,
    /// The row records an undo.
    Restored,
    /// An apply of a store that is not the helper's current one: its journal is not reachable.
    OldStore,
}

/// One mirrored row.
#[derive(Debug, Clone, Copy)]
pub struct MirrorRow<'a> {
    /// The helper store the row came from.
    pub store: &'a str,
    /// The record.
    pub record: &'a ActivityRecord,
    /// Where it stands with respect to undo.
    pub state: RowState,
}

impl HelperMirror {
    /// The highest record id pulled from `store` (0 before the first).
    #[must_use]
    pub fn cursor(&self, store: &str) -> u64 {
        self.stores.get(store).map_or(0, |held| held.cursor)
    }

    /// Notes that `store` is the helper's current one.
    pub fn note_current(&mut self, store: &str) {
        if self.current.as_deref() != Some(store) {
            self.current = Some(store.to_owned());
        }
        self.stores.entry(store.to_owned()).or_default();
    }

    /// Adds records pulled from `store`: those above the cursor, once each. Returns how many were
    /// new.
    pub fn add(&mut self, store: &str, records: Vec<ActivityRecord>) -> usize {
        self.note_current(store);
        let held = self.stores.entry(store.to_owned()).or_default();
        let mut added = 0;
        for record in records {
            if record.id.0 <= held.cursor {
                continue;
            }
            held.cursor = record.id.0;
            held.records.push(record);
            added += 1;
        }
        if held.records.len() > MAX_ROWS_PER_STORE {
            let extra = held.records.len() - MAX_ROWS_PER_STORE;
            held.records.drain(..extra);
        }
        added
    }

    /// Every row of every store, newest first (by time, then by id), with its undo state.
    #[must_use]
    pub fn rows(&self) -> Vec<MirrorRow<'_>> {
        let mut rows = Vec::new();
        for (store, held) in &self.stores {
            let current = self.current.as_deref() == Some(store.as_str());
            for (record, state) in held.records.iter().zip(states_of(&held.records, current)) {
                rows.push(MirrorRow {
                    store,
                    record,
                    state,
                });
            }
        }
        rows.sort_by(|a, b| {
            (b.record.timestamp, b.record.id).cmp(&(a.record.timestamp, a.record.id))
        });
        rows
    }

    /// Whether the helper can undo the tweak now: it has an apply in the current store that no
    /// revert took back.
    #[must_use]
    pub fn can_undo(&self, tweak: &str) -> bool {
        self.rows()
            .iter()
            .any(|row| row.state == RowState::CanUndo && row.record.label.as_deref() == Some(tweak))
    }

    /// Clears the history the way the Undo page's Clear does: rows that can still be undone stay,
    /// the others go. The cursors stay, so nothing cleared is fetched again. Returns how many rows
    /// were removed.
    pub fn clear_rows(&mut self) -> usize {
        let mut removed = 0;
        let current = self.current.clone();
        for (store, held) in &mut self.stores {
            let mut states =
                states_of(&held.records, current.as_deref() == Some(store.as_str())).into_iter();
            let before = held.records.len();
            held.records
                .retain(|_| states.next() == Some(RowState::CanUndo));
            removed += before - held.records.len();
        }
        removed
    }
}

/// The undo state of each record of one store, in order.
fn states_of(records: &[ActivityRecord], current: bool) -> Vec<RowState> {
    let mut states: Vec<RowState> = records
        .iter()
        .map(|record| match record.outcome {
            ActivityOutcome::Applied => RowState::CanUndo,
            ActivityOutcome::Reverted | ActivityOutcome::RolledBack => RowState::Restored,
            // `ActivityOutcome` is non-exhaustive: an outcome this build does not know is not
            // offered for undo.
            _ => RowState::Failed,
        })
        .collect();
    // A revert takes back every earlier apply of the same tweak.
    for (at, record) in records.iter().enumerate() {
        if states[at] != RowState::Restored {
            continue;
        }
        for earlier in 0..at {
            if states[earlier] == RowState::CanUndo && records[earlier].label == record.label {
                states[earlier] = RowState::Undone;
            }
        }
    }
    if !current {
        for state in &mut states {
            if *state == RowState::CanUndo {
                *state = RowState::OldStore;
            }
        }
    }
    states
}

/// Pulls what the helper's log holds after the mirror's cursor, when a session is open (and does
/// nothing, with no prompt, when it is not). Returns how many rows were new.
///
/// # Errors
/// The elevator's error; rows pulled before it are in the mirror.
pub fn pull(elevator: &dyn Elevator, mirror: &mut HelperMirror) -> Result<usize, ElevationError> {
    if !elevator.session_open() {
        return Ok(0);
    }
    // Which store is this? The answer to a read past the end says so without any record.
    let probe = elevator.history(PROBE_AFTER, 1)?;
    mirror.note_current(&probe.store);
    let store = probe.store;
    let mut added = 0;
    for _ in 0..MAX_PAGES {
        let after = mirror.cursor(&store);
        let page = elevator.history(after, HISTORY_MAX_RECORDS)?;
        if page.store != store {
            // The helper changed under us: the next pull starts over with the new store.
            break;
        }
        let empty = page.records.is_empty();
        added += mirror.add(&store, page.records);
        if empty || !page.more {
            break;
        }
    }
    Ok(added)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::activity::ActivityId;
    use crate::elevation::fakes::FakeElevator;
    use crate::fakes::FakeMemory;
    use crate::journal::Source;
    use crate::time::Timestamp;

    fn record(id: u64, outcome: ActivityOutcome, label: &str) -> ActivityRecord {
        ActivityRecord {
            id: ActivityId(id),
            timestamp: Timestamp::from_unix_millis(i64::try_from(id).unwrap_or(0) * 1000),
            source: Source::Manual,
            outcome,
            summary: format!("row {id}"),
            batch: None,
            entry: None,
            label: Some(label.to_owned()),
            phrase: None,
        }
    }

    #[test]
    fn rows_are_added_once_and_the_cursor_follows() {
        let mut mirror = HelperMirror::default();
        let rows = vec![
            record(1, ActivityOutcome::Applied, "a"),
            record(2, ActivityOutcome::Failed, "a"),
        ];
        assert_eq!(mirror.add("s1", rows.clone()), 2);
        assert_eq!(mirror.add("s1", rows), 0);
        assert_eq!(mirror.cursor("s1"), 2);
        assert_eq!(mirror.cursor("other"), 0);
    }

    #[test]
    fn an_apply_can_be_undone_until_a_revert_of_the_same_tweak_follows() {
        let mut mirror = HelperMirror::default();
        mirror.add(
            "s1",
            vec![
                record(1, ActivityOutcome::Applied, "a"),
                record(2, ActivityOutcome::Applied, "b"),
                record(3, ActivityOutcome::Reverted, "a"),
                record(4, ActivityOutcome::Applied, "a"),
            ],
        );
        let states: Vec<_> = mirror
            .rows()
            .iter()
            .map(|row| (row.record.id.0, row.state))
            .collect();
        assert_eq!(
            states,
            vec![
                (4, RowState::CanUndo),
                (3, RowState::Restored),
                (2, RowState::CanUndo),
                (1, RowState::Undone),
            ]
        );
        assert!(mirror.can_undo("a") && mirror.can_undo("b") && !mirror.can_undo("c"));
    }

    #[test]
    fn rows_of_another_store_are_shown_without_undo() {
        let mut mirror = HelperMirror::default();
        mirror.add("old", vec![record(1, ActivityOutcome::Applied, "a")]);
        mirror.add("new", vec![record(1, ActivityOutcome::Failed, "a")]);
        assert_eq!(mirror.current.as_deref(), Some("new"));
        let old = mirror
            .rows()
            .into_iter()
            .find(|row| row.store == "old")
            .map(|row| row.state);
        assert_eq!(old, Some(RowState::OldStore));
        assert!(!mirror.can_undo("a"));
        // The new store's cursor is its own: its first record was not skipped.
        assert_eq!(mirror.cursor("new"), 1);
    }

    #[test]
    fn clearing_keeps_what_can_still_be_undone_and_the_cursors() {
        let mut mirror = HelperMirror::default();
        mirror.add(
            "s1",
            vec![
                record(1, ActivityOutcome::Applied, "a"),
                record(2, ActivityOutcome::Reverted, "a"),
                record(3, ActivityOutcome::Failed, "b"),
                record(4, ActivityOutcome::Applied, "c"),
            ],
        );
        assert_eq!(mirror.clear_rows(), 3);
        let left: Vec<_> = mirror.rows().iter().map(|row| row.record.id.0).collect();
        assert_eq!(left, vec![4]);
        assert_eq!(mirror.cursor("s1"), 4);
    }

    #[test]
    fn the_rows_of_a_store_are_bounded() {
        let mut mirror = HelperMirror::default();
        let many = (1..=(MAX_ROWS_PER_STORE as u64 + 10))
            .map(|id| record(id, ActivityOutcome::Failed, "a"))
            .collect();
        mirror.add("s1", many);
        assert_eq!(mirror.stores["s1"].records.len(), MAX_ROWS_PER_STORE);
        assert_eq!(mirror.stores["s1"].records[0].id.0, 11);
    }

    #[test]
    fn a_pull_does_nothing_without_a_session_and_never_starts_one() {
        let elevator = FakeElevator::granted(Arc::new(FakeMemory::new()));
        let mut mirror = HelperMirror::default();
        assert_eq!(pull(&elevator, &mut mirror), Ok(0));
        assert_eq!(elevator.prompts(), 0);
        assert!(mirror.stores.is_empty());
    }

    #[test]
    fn a_pull_reads_every_page_after_the_cursor() {
        let elevator = FakeElevator::granted(Arc::new(FakeMemory::new()));
        let rows: Vec<_> = (1..=45)
            .map(|id| record(id, ActivityOutcome::Applied, "a"))
            .collect();
        elevator.set_history("s1", rows);
        elevator.hello().expect("session");
        let mut mirror = HelperMirror::default();
        assert_eq!(pull(&elevator, &mut mirror), Ok(45));
        assert_eq!(mirror.cursor("s1"), 45);
        // A second pull finds nothing new.
        assert_eq!(pull(&elevator, &mut mirror), Ok(0));
    }
}
