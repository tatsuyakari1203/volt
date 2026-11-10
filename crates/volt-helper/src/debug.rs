//! Test modes of the binary. This whole module exists only outside release builds (the
//! `cfg(not(helper_release))` on its declaration in `lib.rs`), which is exactly when the binary has no
//! administrator manifest, so a binary that asks for elevation can never carry them. They are
//! selected by the first argument, which the strict parser of a release binary would refuse.
//!
//! - `harden-selftest`: hardens THIS process (unelevated, it is a throwaway child of a test) and
//!   prints what is left, so a test can check the real hardening without hardening itself.
//! - `probe --pipe NAME`: connects to the pipe as a client, says hello and prints whether it was
//!   served, so a test has a second process that is not the one the helper is bound to.

use std::time::Duration;

use volt_core::{
    PROTOCOL_VERSION, ReplyBody, Request, RequestEnvelope, decode_reply, encode_request,
};
use volt_win::helper::args::PipeName;
use volt_win::helper::harden::harden_process;
use volt_win::helper::pipe::PipeClient;

/// Runs a test mode when `argv` selects one; `None` for a normal start.
#[must_use]
pub fn run(argv: &[String]) -> Option<u8> {
    match argv.get(1).map(String::as_str) {
        Some("harden-selftest") => Some(harden_selftest()),
        Some("probe") => Some(probe(argv)),
        _ => None,
    }
}

fn harden_selftest() -> u8 {
    match harden_process() {
        Ok(report) => {
            println!("variables={}", report.variables.join(","));
            println!("directory={}", report.working_directory);
            println!("privileges={}", report.privileges.join(","));
            0
        }
        Err(error) => {
            println!("error={error}");
            1
        }
    }
}

/// Exit code 0 when the helper served this client (it got a `hello` reply), 1 when it did not.
fn probe(argv: &[String]) -> u8 {
    let Some(name) = argv.get(3).and_then(|name| PipeName::parse(name)) else {
        return 2;
    };
    let wait = Some(Duration::from_secs(5));
    let Ok(client) = PipeClient::open_waiting(&name, Duration::from_secs(5)) else {
        println!("refused: could not open");
        return 1;
    };
    let hello = RequestEnvelope {
        id: 1,
        request: Request::Hello {
            protocol: PROTOCOL_VERSION,
        },
    };
    let Ok(bytes) = encode_request(&hello) else {
        return 2;
    };
    if client.write_message(&bytes, wait).is_err() {
        println!("refused: write failed");
        return 1;
    }
    match client
        .read_message(4096, wait)
        .ok()
        .and_then(|reply| decode_reply(&reply).ok())
    {
        Some(reply) if matches!(reply.body, ReplyBody::Hello(_)) => {
            println!("served");
            0
        }
        _ => {
            println!("refused: no reply");
            1
        }
    }
}
