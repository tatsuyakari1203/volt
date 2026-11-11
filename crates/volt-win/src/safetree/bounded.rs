//! Running a walk on a worker thread that can be stopped even while it waits inside the kernel.
//!
//! A directory listing or a delete on a stuck network share or a failing disk can block for as long
//! as the device takes. The walk checks its [`Control`] before every entry, but it cannot do that
//! inside one call. [`run_bounded`] runs the work on its own thread, waits until the deadline, sets
//! the cancel flag, asks Windows to cancel the thread's synchronous call (`CancelSynchronousIo`) and
//! waits a little more. A thread that still has not returned is [`Stuck`]: the caller (the helper)
//! cannot make it stop, so it ends the process, which is the one thing that always works.

use std::os::windows::io::AsRawHandle as _;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::IO::CancelSynchronousIo;

use super::Control;

/// How long a cancelled thread gets to come back.
const GRACE: Duration = Duration::from_secs(5);

/// The work did not stop when it was cancelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stuck;

/// Runs `work` on a new thread. Returns its result, or [`Stuck`] when it has not finished
/// [`GRACE`] after the deadline of `control` and a cancel.
///
/// # Errors
/// [`Stuck`], see above; the thread is left running.
pub fn run_bounded<T, F>(control: &Control, work: F) -> Result<T, Stuck>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let (send, receive) = mpsc::channel();
    let thread = std::thread::Builder::new()
        .name("volt-safetree".to_owned())
        .spawn(move || {
            let _ = send.send(work());
        })
        .map_err(|_| Stuck)?;
    let wait = control.deadline().saturating_duration_since(Instant::now());
    if let Ok(value) = receive.recv_timeout(wait) {
        let _ = thread.join();
        return Ok(value);
    }
    // Out of time: ask the walk to stop, then the kernel call it may be inside.
    control.cancel_flag().store(true, Ordering::SeqCst);
    // SAFETY: the raw handle belongs to `thread`, which is alive (its result has not arrived and
    // the join handle is held here); `CancelSynchronousIo` only asks Windows to cancel the call.
    let _ = unsafe { CancelSynchronousIo(HANDLE(thread.as_raw_handle())) };
    match receive.recv_timeout(GRACE) {
        Ok(value) => {
            let _ = thread.join();
            Ok(value)
        }
        Err(_) => Err(Stuck),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use super::*;

    #[test]
    fn work_that_finishes_returns_its_value() {
        let control = Control::new(Arc::new(AtomicBool::new(false)), Duration::from_secs(5));
        assert_eq!(run_bounded(&control, || 7), Ok(7));
    }

    #[test]
    fn work_that_notices_the_cancel_flag_comes_back_after_the_deadline() {
        let cancel = Arc::new(AtomicBool::new(false));
        let control = Control::new(Arc::clone(&cancel), Duration::from_millis(50));
        let seen = Arc::clone(&cancel);
        let got = run_bounded(&control, move || {
            while !seen.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(5));
            }
            "stopped"
        });
        assert_eq!(got, Ok("stopped"));
    }
}
