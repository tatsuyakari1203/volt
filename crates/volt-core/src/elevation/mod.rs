//! The elevated helper's wire protocol and the [`Elevator`] port.
//!
//! The helper is a second, small process that runs with administrator rights and serves ONE
//! connection of the UI that started it. Nothing here talks to Windows: this module is the
//! vocabulary both sides share, so each side can be tested without the other.
//!
//! # The protocol
//!
//! One request or reply per pipe message, as JSON. A request is a [`RequestEnvelope`]: a `u64`
//! id and a [`Request`] from a CLOSED set (`hello`, `memory_purge`, `tweak_apply`,
//! `tweak_revert`, `history`, `bye`). It carries no path, command, registry location or service
//! name (a tweak is named by its catalogue id and nothing else), so the helper has nothing to be
//! talked into running. The rules that make it hard to abuse, all enforced by [`decode_request`] and
//! [`RequestSequencer`]:
//!
//! - a request is at most [`MAX_REQUEST_BYTES`] (1 KiB);
//! - unknown fields are refused, not ignored (`deny_unknown_fields`);
//! - an unknown request type is refused;
//! - ids must be exactly 1, 2, 3, ...: anything else is [`ErrorCode::OutOfSequence`].
//!
//! Every refusal has an [`ErrorCode`]; [`ErrorCode::closes_connection`] says whether the helper
//! hangs up after sending it.
//!
//! # The port
//!
//! The UI talks to the helper through the [`Elevator`] trait. The real implementation (start the
//! helper through UAC, connect, authenticate the server) is a Windows adapter; [`fakes::FakeElevator`]
//! stands in for it in tests, so UI code can be built and tested with no helper at all.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::activity::ActivityRecord;
use crate::error::PortError;
use crate::memory::{AreaReport, MemoryCleanReport};
use crate::ports::{MemoryArea, MemoryLists, PurgeOutcome};
use crate::tweak::ApplyOutcome;

pub mod confirm;
#[cfg(any(test, feature = "fakes"))]
pub mod fakes;
#[cfg(test)]
mod tests;

/// The protocol version both sides must agree on in `hello`.
pub const PROTOCOL_VERSION: u32 = 1;

/// Longest request the helper accepts, in bytes.
pub const MAX_REQUEST_BYTES: usize = 1024;

/// Longest reply the helper sends, in bytes.
pub const MAX_REPLY_BYTES: usize = 4096;

/// A request with its id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestEnvelope {
    /// 1 for the first request of a connection, then one more each time.
    pub id: u64,
    /// What is asked.
    pub request: Request,
}

/// The closed set of requests of this protocol version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    /// Must be first: agrees on the protocol version and asks for the catalogue hashes.
    Hello {
        /// The protocol version the UI speaks.
        protocol: u32,
    },
    /// Purge memory areas (the helper holds the privileges the list purges need).
    MemoryPurge {
        /// The areas, each at most once.
        areas: Vec<MemoryArea>,
    },
    /// Applies one administrator tweak of the helper's own catalogue, named by its id. A change
    /// that persists, so the helper asks for confirmation on its own before it acts.
    TweakApply {
        /// The tweak id.
        tweak: String,
    },
    /// Reverts what the helper applied for this tweak, from the helper's own journal. Confirmed
    /// like an apply.
    TweakRevert {
        /// The tweak id.
        tweak: String,
    },
    /// Reads the helper's activity log, oldest first, so the UI can mirror it by pull: records
    /// with an id above `after` (0 for all), at most `limit` of them (the helper may send fewer
    /// to fit a reply).
    History {
        /// The last record id the UI already holds.
        after: u64,
        /// The most records to send.
        limit: u32,
    },
    /// Ends the session: the helper replies and exits.
    Bye {},
}

/// The request types of this version, as they appear in the `type` field.
const REQUEST_TYPES: [&str; 6] = [
    "hello",
    "memory_purge",
    "tweak_apply",
    "tweak_revert",
    "history",
    "bye",
];

/// The most records one `history` reply may carry, whatever `limit` asks for.
pub const HISTORY_MAX_RECORDS: u32 = 20;

/// A reply with the id of the request it answers (0 when the request had no readable id).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reply {
    /// The id of the request.
    pub id: u64,
    /// The answer.
    pub body: ReplyBody,
}

/// What the helper answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReplyBody {
    /// Answer to `hello`.
    Hello(HelloReply),
    /// Answer to `memory_purge`.
    MemoryPurge(MemoryPurgeReply),
    /// Answer to `tweak_apply`.
    TweakApply(TweakApplyReply),
    /// Answer to `tweak_revert`.
    TweakRevert(TweakRevertReply),
    /// Answer to `history`.
    History(HistoryReply),
    /// Answer to `bye`.
    Bye {},
    /// The request was refused.
    Error {
        /// Why.
        code: ErrorCode,
    },
}

/// Answer to `hello`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloReply {
    /// The protocol version the helper speaks (equal to the request's, or the helper refused).
    pub protocol: u32,
    /// The helper's own version.
    pub helper_version: String,
    /// Hashes of the catalogues the helper holds. The UI refuses a helper whose hashes differ
    /// from its own.
    pub catalogues: CatalogueHashes,
}

/// Fingerprints of the catalogue files, see [`catalogue_hash`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogueHashes {
    /// The tweak catalogue.
    pub tweaks: String,
    /// The cleaning rules.
    pub cleaning: String,
}

/// The SHA-256 of a catalogue text, as lower-case hex. Both sides hash the text they embed, so
/// equal hashes mean the helper and the UI hold the same catalogue.
#[must_use]
pub fn catalogue_hash(text: &str) -> String {
    use std::fmt::Write as _;
    let digest = Sha256::digest(text.as_bytes());
    let mut hex = String::with_capacity(64);
    for byte in digest {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// Answer to `tweak_apply`: how the apply ended. Failures that changed nothing come back as an
/// error code instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TweakApplyReply {
    /// The tweak id.
    pub tweak: String,
    /// How the apply ended.
    pub outcome: ApplyOutcome,
}

/// Answer to `tweak_revert`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TweakRevertReply {
    /// The tweak id.
    pub tweak: String,
    /// How many journal batches were reverted.
    pub batches: u32,
    /// Whether everything that was attempted was restored.
    pub complete: bool,
}

/// Answer to `history`: records of the helper's activity log, oldest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryReply {
    /// Identifies the helper's store. The UI keys what it mirrors by (`store`, record id) and
    /// keeps one cursor per store: a store that was recreated has a new identifier, and its
    /// older rows stay visible without Undo.
    pub store: String,
    /// The records after the requested id.
    pub records: Vec<ActivityRecord>,
    /// Whether the log holds more records after the last one sent.
    pub more: bool,
}

/// How one area ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum AreaStatus {
    /// The area was purged.
    Purged {
        /// Processes whose working set was trimmed (working sets only).
        processes_trimmed: u32,
        /// Processes that could not be opened and were left.
        processes_skipped: u32,
    },
    /// The purge did not run or failed; nothing was changed for this area.
    Failed {
        /// Why.
        error: PortError,
    },
}

/// One area of a purge and how it ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AreaResult {
    /// The area.
    pub area: MemoryArea,
    /// How it ended.
    #[serde(flatten)]
    pub status: AreaStatus,
}

/// Answer to `memory_purge`: memory by list before and after, and one result per area.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryPurgeReply {
    /// Memory by list before the first purge, when it could be read.
    pub before: Option<MemoryLists>,
    /// Memory by list after the last purge, when it could be read.
    pub after: Option<MemoryLists>,
    /// One entry per area that ran, in the order they ran.
    pub areas: Vec<AreaResult>,
}

impl MemoryPurgeReply {
    /// Whether at least one area was purged.
    #[must_use]
    pub fn any_purged(&self) -> bool {
        self.areas
            .iter()
            .any(|area| matches!(area.status, AreaStatus::Purged { .. }))
    }
}

impl From<&MemoryCleanReport> for MemoryPurgeReply {
    fn from(report: &MemoryCleanReport) -> Self {
        Self {
            before: report.before,
            after: report.after,
            areas: report
                .areas
                .iter()
                .map(|entry| AreaResult {
                    area: entry.area,
                    status: match &entry.result {
                        Ok(outcome) => AreaStatus::Purged {
                            processes_trimmed: outcome.processes_trimmed,
                            processes_skipped: outcome.processes_skipped,
                        },
                        Err(error) => AreaStatus::Failed {
                            error: error.clone(),
                        },
                    },
                })
                .collect(),
        }
    }
}

impl From<&MemoryPurgeReply> for MemoryCleanReport {
    /// The report of a purge the helper ran, in the form the activity record is written from. The
    /// record has not been written yet, so `activity_write_failed` is `false`.
    fn from(reply: &MemoryPurgeReply) -> Self {
        Self {
            before: reply.before,
            after: reply.after,
            areas: reply
                .areas
                .iter()
                .map(|entry| AreaReport {
                    area: entry.area,
                    result: match &entry.status {
                        AreaStatus::Purged {
                            processes_trimmed,
                            processes_skipped,
                        } => Ok(PurgeOutcome {
                            processes_trimmed: *processes_trimmed,
                            processes_skipped: *processes_skipped,
                            // The helper never sees the exclusion list: the app does not send
                            // working-set trims to it.
                            processes_excluded: 0,
                        }),
                        AreaStatus::Failed { error } => Err(error.clone()),
                    },
                })
                .collect(),
            activity_write_failed: false,
        }
    }
}

/// Why the helper refused a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Error, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// The request was longer than [`MAX_REQUEST_BYTES`].
    #[error("request too large")]
    TooLarge,
    /// Not valid JSON of the expected shape, or it has a field the protocol does not know.
    #[error("malformed request")]
    Malformed,
    /// A well-formed request whose type is not in the closed set.
    #[error("unknown request")]
    UnknownRequest,
    /// The id was not the previous id plus one.
    #[error("request id out of sequence")]
    OutOfSequence,
    /// The UI and the helper speak different protocol versions.
    #[error("protocol version mismatch")]
    ProtocolMismatch,
    /// A request other than `hello` or `bye` came before `hello`.
    #[error("hello required first")]
    HelloRequired,
    /// A valid request with an argument that is not acceptable (no areas, a repeated area, a
    /// second hello).
    #[error("invalid request")]
    InvalidRequest,
    /// Too many requests of this kind lately.
    #[error("rate limited")]
    RateLimited,
    /// The helper failed in a way that is not the request's fault.
    #[error("internal error")]
    Internal,
    /// The tweak id is not an administrator tweak of the helper's catalogue.
    #[error("unknown administrator tweak")]
    UnknownTweak,
    /// The change needs a confirmation and nobody accepted it (it was declined, or the helper
    /// has no way to ask). Nothing was changed.
    #[error("not confirmed")]
    NotConfirmed,
    /// The same request was declined lately (or confirmations are locked for this session), so it
    /// was refused without asking. Nothing was changed.
    #[error("blocked after a declined confirmation")]
    Blocked,
    /// There is no change of this tweak that the helper's journal can revert.
    #[error("nothing to revert")]
    NothingToRevert,
    /// The tweak could not be carried out (a step could not be read, the journal refused, a
    /// step failed). The details are in the helper's activity log.
    #[error("the tweak could not be carried out")]
    TweakFailed,
    /// The helper cannot serve this kind of request (its state could not be opened).
    #[error("not available in this helper")]
    Unavailable,
}

impl ErrorCode {
    /// Whether the helper hangs up after sending this refusal. A peer that breaks the framing or
    /// the id order is not talking this protocol; a peer that merely asked for something not
    /// acceptable right now keeps its connection.
    #[must_use]
    pub fn closes_connection(self) -> bool {
        !matches!(
            self,
            Self::HelloRequired
                | Self::InvalidRequest
                | Self::RateLimited
                | Self::UnknownTweak
                | Self::NotConfirmed
                | Self::Blocked
                | Self::NothingToRevert
                | Self::TweakFailed
                | Self::Unavailable
        )
    }
}

/// Parses one request message. The checks are in this order: size, JSON shape, request type,
/// the typed parse (which refuses unknown fields).
///
/// # Errors
/// [`ErrorCode::TooLarge`], [`ErrorCode::UnknownRequest`] or [`ErrorCode::Malformed`].
pub fn decode_request(bytes: &[u8]) -> Result<RequestEnvelope, ErrorCode> {
    if bytes.len() > MAX_REQUEST_BYTES {
        return Err(ErrorCode::TooLarge);
    }
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| ErrorCode::Malformed)?;
    if let Some(kind) = value
        .get("request")
        .and_then(|request| request.get("type"))
        .and_then(serde_json::Value::as_str)
        && !REQUEST_TYPES.contains(&kind)
    {
        return Err(ErrorCode::UnknownRequest);
    }
    serde_json::from_value(value).map_err(|_| ErrorCode::Malformed)
}

/// Writes one request message.
///
/// # Errors
/// [`ErrorCode::TooLarge`] when the message would exceed [`MAX_REQUEST_BYTES`] (the helper would
/// refuse it), [`ErrorCode::Internal`] when it cannot be encoded.
pub fn encode_request(envelope: &RequestEnvelope) -> Result<Vec<u8>, ErrorCode> {
    let bytes = serde_json::to_vec(envelope).map_err(|_| ErrorCode::Internal)?;
    if bytes.len() > MAX_REQUEST_BYTES {
        return Err(ErrorCode::TooLarge);
    }
    Ok(bytes)
}

/// Writes one reply message. A reply that would not fit [`MAX_REPLY_BYTES`] becomes an
/// [`ErrorCode::Internal`] reply, so the message the peer reads is always bounded.
#[must_use]
pub fn encode_reply(reply: &Reply) -> Vec<u8> {
    if let Ok(bytes) = serde_json::to_vec(reply)
        && bytes.len() <= MAX_REPLY_BYTES
    {
        return bytes;
    }
    let fallback = Reply {
        id: reply.id,
        body: ReplyBody::Error {
            code: ErrorCode::Internal,
        },
    };
    // A fixed shape of a few dozen bytes: encoding it cannot fail, but never panic over it.
    serde_json::to_vec(&fallback).unwrap_or_default()
}

/// Parses one reply message.
///
/// # Errors
/// [`ErrorCode::TooLarge`] or [`ErrorCode::Malformed`].
pub fn decode_reply(bytes: &[u8]) -> Result<Reply, ErrorCode> {
    if bytes.len() > MAX_REPLY_BYTES {
        return Err(ErrorCode::TooLarge);
    }
    serde_json::from_slice(bytes).map_err(|_| ErrorCode::Malformed)
}

/// Checks that request ids go 1, 2, 3, ... with no gap, repeat or reorder.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RequestSequencer {
    last: u64,
}

impl RequestSequencer {
    /// A sequencer for a new connection: the next id must be 1.
    #[must_use]
    pub const fn new() -> Self {
        Self { last: 0 }
    }

    /// Accepts `id` when it is the next one.
    ///
    /// # Errors
    /// [`ErrorCode::OutOfSequence`]; the sequencer is unchanged.
    pub fn accept(&mut self, id: u64) -> Result<(), ErrorCode> {
        if self.last.checked_add(1) == Some(id) {
            self.last = id;
            Ok(())
        } else {
            Err(ErrorCode::OutOfSequence)
        }
    }

    /// The id of the last accepted request (0 before the first).
    #[must_use]
    pub const fn last(&self) -> u64 {
        self.last
    }
}

/// Why an [`Elevator`] call did not give an answer.
#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ElevationError {
    /// The user declined the administrator prompt. Nothing was changed.
    #[error("the administrator prompt was declined")]
    Declined,
    /// Administrator features are not available on this machine or install, with the reason in
    /// words the UI can show.
    #[error("administrator features are not available: {0}")]
    Unavailable(String),
    /// The helper refused the request.
    #[error("the helper refused the request: {0}")]
    Refused(ErrorCode),
    /// The helper is not the one this build expects (another protocol version or catalogue). No
    /// request was sent.
    #[error("the helper does not match this version of Volt: {0}")]
    Mismatch(String),
    /// The process that serves the pipe could not be shown to be the helper that was started (it
    /// is another process, or the pipe is not owned by Administrators or SYSTEM). No request was
    /// sent.
    #[error("the helper could not be verified: {0}")]
    Untrusted(String),
    /// The session ended before the answer arrived.
    #[error("the administrator session ended")]
    Disconnected,
}

/// Why administrator features cannot be used, one typed reason each so the UI can say it in its
/// own words. None of them starts a prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum UnavailableReason {
    /// Volt is not installed for all users in the Program Files folder (a per-user, portable or
    /// development copy). It never starts the helper from a folder a user can write to.
    NotAllUsersInstall,
    /// The helper executable is not next to the app.
    HelperMissing,
    /// The install folder, the app or the helper can be changed by a non-administrator (or could
    /// not be checked), so the helper must not be started from it.
    InstallNotProtected {
        /// What is wrong, for the log and a details line.
        detail: String,
    },
    /// The account is a standard user: Windows would ask for an administrator's credentials, so
    /// no prompt is started and the user is told to ask an administrator.
    StandardUser,
    /// Anything else (the account's token could not be read, for example).
    Unknown {
        /// What happened.
        detail: String,
    },
}

impl std::fmt::Display for UnavailableReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAllUsersInstall => {
                f.write_str("Administrator features need Volt to be installed for all users.")
            }
            Self::HelperMissing => {
                f.write_str("The administrator helper is missing from the install folder.")
            }
            Self::InstallNotProtected { detail } => write!(
                f,
                "The install folder is not protected against changes by other programs ({detail})."
            ),
            Self::StandardUser => f.write_str(
                "This account cannot approve administrator actions. Ask an administrator.",
            ),
            Self::Unknown { detail } => f.write_str(detail),
        }
    }
}

/// Whether administrator features can be used now, decided without starting anything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ElevationStatus {
    /// A shield action may start the helper (one administrator prompt, or none when the user's own
    /// policy gives none).
    Available,
    /// It cannot, for this reason.
    Unavailable {
        /// Why.
        reason: UnavailableReason,
    },
}

/// Port: administrator-level actions through the elevated helper.
///
/// The first call of a session may show the administrator prompt; later calls reuse the session.
/// An implementation authenticates the helper before it sends anything (see decision on the
/// helper), so a returned `Ok` means the answer came from the real helper.
pub trait Elevator: Send + Sync {
    /// Whether administrator features can be used now. Reads only: it never starts the helper and
    /// never shows a prompt.
    fn status(&self) -> ElevationStatus;

    /// Starts the session if needed, agrees on the protocol and returns what the helper holds.
    fn hello(&self) -> Result<HelloReply, ElevationError>;

    /// Purges memory areas in the helper.
    fn memory_purge(&self, areas: &[MemoryArea]) -> Result<MemoryPurgeReply, ElevationError>;

    /// Applies the administrator tweak `tweak` in the helper, which confirms it on its own first.
    /// A refusal ([`ErrorCode::NotConfirmed`], [`ErrorCode::Blocked`]) changed nothing.
    fn tweak_apply(&self, tweak: &str) -> Result<TweakApplyReply, ElevationError>;

    /// Reverts the helper's own record of the tweak, confirmed like an apply.
    fn tweak_revert(&self, tweak: &str) -> Result<TweakRevertReply, ElevationError>;

    /// Reads the helper's activity log after the record `after` (0 for the start).
    fn history(&self, after: u64, limit: u32) -> Result<HistoryReply, ElevationError>;

    /// Ends the session. The next call starts a new one (and may prompt again).
    fn bye(&self);
}
