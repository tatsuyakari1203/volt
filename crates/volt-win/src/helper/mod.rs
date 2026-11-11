//! Windows side of the elevated helper (see the `volt-core` `elevation` module for the protocol).
//!
//! Everything here is an adapter with no business rules: a named pipe that only the right peer can
//! use ([`pipe`]), facts about processes and tokens and the decision whether a client may be served
//! ([`identity`]), the check that the install folder cannot be tampered with by a non-administrator
//! ([`install`]), the hardening the helper applies to itself at start ([`harden`]) and the strict
//! command line ([`args`]). The helper binary (`volt-helper`) composes them; the UI side (see
//! below) uses the client half of the pipe and the install check.
//!
//! The UI's half of the helper lives in [`launch`] (can the helper be started, and the one call
//! that starts it through the administrator prompt) and [`elevator`] (the session: authenticate the
//! server, agree on the version, send requests).
//!
//! With the `elevated-ci` feature (never enabled by a product build) the module `ci` adds the
//! measurements of the job that runs the real helper on a hosted runner.
//!
//! Only [`launch`] can start a process or show a prompt, and only when the UI asks it to; nothing
//! else in this module does, and tests replace the launch call.

pub mod acl;
pub mod args;
#[cfg(feature = "elevated-ci")]
pub mod ci;
pub mod elevator;
pub mod harden;
pub mod identity;
pub mod install;
pub mod launch;
pub mod pipe;
pub mod uac;

use windows::Win32::Foundation::{CloseHandle, HANDLE};

/// A kernel handle closed on drop.
#[derive(Debug)]
pub(crate) struct OwnedHandle(pub(crate) HANDLE);

impl OwnedHandle {
    pub(crate) fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            // SAFETY: the handle is owned by this value, was returned by a successful call and is
            // closed exactly once, here. A failure leaves nothing more to do.
            let _ = unsafe { CloseHandle(self.0) };
        }
    }
}

// SAFETY: a kernel handle is a process-wide token, not tied to the thread that made it, and the
// kernel serialises operations on it. Moving the owner to another thread is sound.
unsafe impl Send for OwnedHandle {}

/// Null-terminated UTF-16 copy of a string.
pub(crate) fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// `what: error`, the form every failure message here takes.
pub(crate) fn failed(what: &str, error: &windows::core::Error) -> String {
    format!("{what}: {error}")
}
