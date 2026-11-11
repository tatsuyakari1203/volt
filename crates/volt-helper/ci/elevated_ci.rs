//! `volt-elevated-ci`: the harness of the elevated CI job. It exists only with the `elevated-ci`
//! cargo feature, is never built by the release workflow, never bundled and not part of
//! `pnpm check`.
//!
//! The CI job copies this program as `volt-desktop.exe` next to the release `volt-helper.exe` in
//! `C:\Program Files\Volt` on a hosted Windows runner (a throwaway virtual machine whose user holds
//! an administrator token), and runs `volt-desktop.exe run --out <dir>`. The harness then plays the
//! UI with the REAL client path: the real `WinElevator` (availability check, launch through
//! `runas`, server identity, pipe owner, hello, purge, bye) and the real launch call. It records
//! the measurements the decision asks for and runs the negative cases for real.
//!
//! It REFUSES to run anywhere else: it needs `GITHUB_ACTIONS=true`, `RUNNER_ENVIRONMENT=github-hosted`
//! and `VOLT_ELEVATED_CI=1`, which only the workflow sets. Never run it on a developer machine.

#![cfg(windows)]

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

use volt_core::{
    CatalogueHashes, ElevationError, ElevationStatus, Elevator, MemoryArea, MemoryPurgeReply,
    PROTOCOL_VERSION, ReplyBody, Request, RequestEnvelope, UnavailableReason, decode_reply,
    encode_request,
};
use volt_helper::run::exit;
use volt_win::WinElevator;
use volt_win::helper::args::{HelperArgs, PipeName};
use volt_win::helper::ci;
use volt_win::helper::elevator::ConnectionEvidence;
use volt_win::helper::identity::{ProcessFacts, current_token_kind, inspect_current};
use volt_win::helper::launch::{HELPER_IMAGE_NAME, status_for};
use volt_win::helper::pipe::{OpenError, PipeClient, is_trusted_owner};

/// Exit code of a refusal to run outside a hosted runner.
const REFUSED: u8 = 64;
const SHORT: Option<Duration> = Some(Duration::from_secs(10));
/// The purge quota is one per 5 seconds; wait a little longer.
const PURGE_GAP: Duration = Duration::from_secs(6);
/// SID of the built-in Users group, for the writable-folder case.
const USERS_SID: &str = "*S-1-5-32-545";

fn main() -> ExitCode {
    if let Err(why) = hosted_runner_only() {
        eprintln!("volt-elevated-ci refuses to run: {why}");
        eprintln!(
            "This harness starts the elevated helper and purges memory. It runs only in the \
             elevated CI job on a GitHub-hosted runner, never on a developer machine."
        );
        return ExitCode::from(REFUSED);
    }
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("run") if args.get(2).map(String::as_str) == Some("--out") => match args.get(3) {
            Some(out) => ExitCode::from(run(Path::new(out))),
            None => ExitCode::from(2),
        },
        Some("rogue") => ExitCode::from(args.get(2).map_or(2, |pipe| rogue(pipe))),
        _ => {
            eprintln!("usage: volt-desktop.exe run --out <folder>");
            ExitCode::from(2)
        }
    }
}

/// The three facts only a GitHub-hosted runner running this workflow has.
fn hosted_runner_only() -> Result<(), String> {
    check_environment(|name| std::env::var(name).ok())
}

fn check_environment(get: impl Fn(&str) -> Option<String>) -> Result<(), String> {
    for (name, value) in [
        ("GITHUB_ACTIONS", "true"),
        ("RUNNER_ENVIRONMENT", "github-hosted"),
        ("VOLT_ELEVATED_CI", "1"),
    ] {
        if get(name).as_deref() != Some(value) {
            return Err(format!("{name} is not {value}"));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Report

#[derive(Default)]
struct Report {
    checks: Vec<(String, bool, String)>,
    facts: Vec<(String, String)>,
}

impl Report {
    fn check(&mut self, name: &str, ok: bool, detail: impl Into<String>) {
        let detail = detail.into();
        println!("[{}] {name}: {detail}", if ok { "PASS" } else { "FAIL" });
        self.checks.push((name.to_owned(), ok, detail));
    }

    fn fact(&mut self, name: &str, value: impl Into<String>) {
        let value = value.into();
        println!("[fact] {name} = {value}");
        self.facts.push((name.to_owned(), value));
    }

    fn all_passed(&self) -> bool {
        self.checks.iter().all(|(_, ok, _)| *ok)
    }

    fn markdown(&self) -> String {
        let cell = |text: &str| text.replace('|', "\\|").replace('\n', " ");
        let mut md = String::from(
            "## Elevated helper, real run\n\n\
             Scope: the runner's user already holds a full administrator token, so `runas` starts \
             the helper without crossing a consent prompt. The rights of the launch handle seen \
             here are those of an elevated-to-elevated launch; a launch from a filtered \
             (medium-integrity) token is not measured by this job.\n\n### Checks\n\n",
        );
        md.push_str("| Check | Result | Detail |\n| --- | --- | --- |\n");
        for (name, ok, detail) in &self.checks {
            let _ = writeln!(
                md,
                "| {} | {} | {} |",
                cell(name),
                if *ok { "pass" } else { "FAIL" },
                cell(detail)
            );
        }
        md.push_str("\n### Measurements\n\n| Measurement | Value |\n| --- | --- |\n");
        for (name, value) in &self.facts {
            let _ = writeln!(md, "| {} | {} |", cell(name), cell(value));
        }
        md
    }

    fn json(&self) -> String {
        let mut out = String::from("{\n  \"passed\": ");
        out.push_str(if self.all_passed() { "true" } else { "false" });
        out.push_str(",\n  \"checks\": [");
        for (index, (name, ok, detail)) in self.checks.iter().enumerate() {
            let _ = write!(
                out,
                "{}\n    {{\"name\": {}, \"ok\": {ok}, \"detail\": {}}}",
                if index == 0 { "" } else { "," },
                quote(name),
                quote(detail)
            );
        }
        out.push_str("\n  ],\n  \"measurements\": {");
        for (index, (name, value)) in self.facts.iter().enumerate() {
            let _ = write!(
                out,
                "{}\n    {}: {}",
                if index == 0 { "" } else { "," },
                quote(name),
                quote(value)
            );
        }
        out.push_str("\n  }\n}\n");
        out
    }
}

/// A JSON string literal.
fn quote(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if u32::from(c) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn millis(duration: Duration) -> String {
    format!("{:.1} ms", duration.as_secs_f64() * 1000.0)
}

fn mib(bytes: u64) -> String {
    #[allow(
        clippy::cast_precision_loss,
        reason = "a working set far below 2^52 bytes, shown to one decimal"
    )]
    let value = bytes as f64 / (1024.0 * 1024.0);
    format!("{value:.2} MiB ({bytes} bytes)")
}

// ---------------------------------------------------------------------------------------------
// The run

fn run(out: &Path) -> u8 {
    let _ = std::fs::create_dir_all(out);
    let mut report = Report::default();
    match inspect_current() {
        Ok(me) => {
            let image = PathBuf::from(&me.identity.image);
            report.fact("harness image", me.identity.image.clone());
            report.fact("user SID", me.token.user_sid.clone());
            report.fact(
                "harness integrity level",
                format!("{:#x}", me.token.integrity_rid),
            );
            report.fact("session", me.token.session_id.to_string());
            report.fact(
                "token kind seen by the availability check",
                format!("{:?}", current_token_kind()),
            );
            real_session(&mut report);
            bound_client_only(&mut report, &image, &me);
            negative_cases(&mut report, &image, &me);
            abandoned_helper(&mut report, &image, &me);
        }
        Err(error) => report.check("identify the harness process", false, error),
    }
    let _ = std::fs::write(out.join("elevated-helper-report.md"), report.markdown());
    let _ = std::fs::write(out.join("elevated-helper-report.json"), report.json());
    u8::from(!report.all_passed())
}

fn describe_evidence(report: &mut Report, label: &str, evidence: &ConnectionEvidence) {
    report.fact(
        &format!("{label}: pipe owner SID"),
        format!("{:?}", evidence.owner_sid),
    );
    report.fact(
        &format!("{label}: pipe owner trusted by the client"),
        evidence.owner_trusted.to_string(),
    );
    report.fact(
        &format!("{label}: GetNamedPipeServerProcessId"),
        format!("{:?}", evidence.server_pid),
    );
    report.fact(
        &format!("{label}: launch handle process id (GetProcessId)"),
        format!("{:?}", evidence.helper_pid),
    );
    report.fact(
        &format!("{label}: server process id matches the launch handle"),
        evidence.server_is_launched.to_string(),
    );
    let rights = &evidence.rights;
    report.fact(
        &format!("{label}: launch handle access mask"),
        rights
            .granted_access
            .map_or_else(|| "unreadable".to_owned(), |mask| format!("{mask:#010x}")),
    );
    report.fact(
        &format!("{label}: launch handle can be waited on"),
        rights.can_wait.to_string(),
    );
    report.fact(
        &format!("{label}: launch handle GetProcessId works"),
        rights.process_id.is_some().to_string(),
    );
    report.fact(
        &format!("{label}: launch handle GetExitCodeProcess works"),
        rights.can_query_exit_code.to_string(),
    );
    let memory = match &evidence.working_set {
        Ok(set) => format!(
            "working set {}, peak {}, private {}",
            mib(set.working_set),
            mib(set.peak_working_set),
            mib(set.private_bytes)
        ),
        Err(error) => format!("unreadable ({error})"),
    };
    report.fact(&format!("{label}: helper memory"), memory);
}

/// The whole happy path through the real `WinElevator`, then bye and the helper's exit.
fn real_session(report: &mut Report) {
    let elevator = WinElevator::new(volt_helper::catalogue_hashes());

    let status = elevator.status();
    report.check(
        "availability check says Available for this install",
        status == ElevationStatus::Available,
        format!("{status:?}"),
    );

    let hello = elevator.hello();
    match &hello {
        Ok(hello) => report.check(
            "launch through runas, authenticate the server, hello",
            hello.protocol == PROTOCOL_VERSION
                && hello.catalogues == volt_helper::catalogue_hashes(),
            format!(
                "helper {} speaks protocol {} with this build's catalogue hashes",
                hello.helper_version, hello.protocol
            ),
        ),
        Err(error) => {
            report.check(
                "launch through runas, authenticate the server, hello",
                false,
                error.to_string(),
            );
            return;
        }
    }
    let Some(first) = elevator.ci_evidence() else {
        report.check("the connection is live after hello", false, "no connection");
        return;
    };
    describe_evidence(report, "after hello", &first);
    report.fact("time of the launch call", millis(first.launch_call));
    report.fact(
        "time from launch to the hello reply",
        millis(first.launch_to_hello),
    );
    report.check(
        "pipe owner is Administrators or SYSTEM",
        first.owner_sid.as_deref().is_ok_and(is_trusted_owner),
        format!("{:?}", first.owner_sid),
    );
    report.check(
        "server process id equals the launch handle's process id",
        first.server_is_launched,
        format!(
            "server {:?}, launch handle {:?}",
            first.server_pid, first.helper_pid
        ),
    );

    purge_and_bye(report, &elevator, &first);
}

/// Two purges on one helper, a stranger in between, then bye and the helper's exit.
fn purge_and_bye(report: &mut Report, elevator: &WinElevator, first: &ConnectionEvidence) {
    // First purge.
    let purge = elevator.memory_purge(&MemoryArea::RUN_ORDER);
    match &purge {
        Ok(reply) => {
            report.check(
                "memory purge succeeds",
                reply.any_purged(),
                format!("{:?}", reply.areas),
            );
            report.fact(
                "memory lists before the first purge",
                format!("{:?}", reply.before),
            );
            report.fact(
                "memory lists after the first purge",
                format!("{:?}", reply.after),
            );
        }
        Err(error) => report.check("memory purge succeeds", false, error.to_string()),
    }
    report.fact(
        "purge succeeded after the helper hardened itself",
        purge
            .as_ref()
            .is_ok_and(MemoryPurgeReply::any_purged)
            .to_string(),
    );
    if let Some(after_purge) = elevator.ci_evidence() {
        describe_evidence(report, "after the first purge", &after_purge);
    }

    // A second process while the bound client is connected: the pipe has one instance, so it
    // cannot even connect, and the bound client is still served afterwards.
    let (code, text) = run_rogue(first.pipe.as_str());
    report.check(
        "a second process is refused while the bound client is served",
        code == Some(1) && text.contains("refused"),
        format!("exit {code:?}, said {text:?}"),
    );

    // Second purge on the same helper, no relaunch.
    std::thread::sleep(PURGE_GAP);
    let again = elevator.memory_purge(&MemoryArea::RUN_ORDER);
    let same_helper = elevator
        .ci_evidence()
        .is_some_and(|now| now.helper_pid == first.helper_pid && now.pipe == first.pipe);
    report.check(
        "a second purge works without a relaunch",
        again.as_ref().is_ok_and(MemoryPurgeReply::any_purged) && same_helper,
        format!(
            "{}; same helper process and pipe: {same_helper}",
            again
                .as_ref()
                .map_or_else(ToString::to_string, |reply| format!("{:?}", reply.areas))
        ),
    );

    // Bye, and the helper exits.
    let waitable = first.helper_pid.map(ci::open_waitable);
    let started = Instant::now();
    elevator.bye();
    let code = match &waitable {
        Some(Ok(process)) => ci::wait_exit_code(process, Duration::from_secs(20)),
        _ => None,
    };
    report.check(
        "after bye the helper exits with code 0",
        code == Some(0),
        format!("exit code {code:?} after {}", millis(started.elapsed())),
    );
}

/// A second process that is not the bound client, run as a child of this one.
fn run_rogue(pipe: &str) -> (Option<i32>, String) {
    let Ok(me) = std::env::current_exe() else {
        return (None, "no current exe".to_owned());
    };
    match Command::new(me).args(["rogue", pipe]).output() {
        Ok(output) => (
            output.status.code(),
            String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        ),
        Err(error) => (None, error.to_string()),
    }
}

/// The child's side: connect to the pipe and say hello. Exit 1 when the helper does not serve it.
fn rogue(pipe: &str) -> u8 {
    let Some(name) = PipeName::parse(pipe) else {
        return 2;
    };
    let client = match PipeClient::open_waiting(&name, Duration::from_secs(3)) {
        Ok(client) => client,
        Err(OpenError::Busy) => {
            println!("refused: the single pipe instance is busy");
            return 1;
        }
        Err(other) => {
            println!("refused: could not open ({other:?})");
            return 1;
        }
    };
    match hello_over(&client) {
        Ok(()) => {
            println!("served");
            0
        }
        Err(error) => {
            println!("refused: {error}");
            1
        }
    }
}

/// One `hello` request and its answer, by hand (the rogue and the bound client of the second
/// scenario do not go through `WinElevator`).
fn hello_over(client: &PipeClient) -> Result<(), String> {
    request_over(
        client,
        1,
        Request::Hello {
            protocol: PROTOCOL_VERSION,
        },
    )
    .and_then(|body| match body {
        ReplyBody::Hello(_) => Ok(()),
        other => Err(format!("unexpected answer {other:?}")),
    })
}

fn request_over(client: &PipeClient, id: u64, request: Request) -> Result<ReplyBody, String> {
    let bytes = encode_request(&RequestEnvelope { id, request }).map_err(|e| e.to_string())?;
    client
        .write_message(&bytes, SHORT)
        .map_err(|e| format!("write failed ({e})"))?;
    let reply = client
        .read_message(volt_core::MAX_REPLY_BYTES, SHORT)
        .map_err(|e| format!("no reply ({e})"))?;
    decode_reply(&reply)
        .map(|reply| reply.body)
        .map_err(|e| format!("unreadable reply ({e})"))
}

/// Starts a helper, lets a stranger connect FIRST (it must be hung up on), then the bound client
/// (this process), which must be served by the same helper.
fn bound_client_only(report: &mut Report, image: &Path, me: &ProcessFacts) {
    let Ok(pipe) = PipeName::random() else {
        report.check(
            "a stranger is refused, the bound client is served",
            false,
            "no pipe name",
        );
        return;
    };
    let helper = image.with_file_name(HELPER_IMAGE_NAME);
    let args = HelperArgs {
        pipe: pipe.clone(),
        ui_pid: me.identity.pid,
        ui_created_at_100ns: me.identity.created_at_100ns,
    }
    .render();
    let process = match ci::launch(&helper, &args) {
        Ok(process) => process,
        Err(error) => {
            report.check(
                "a stranger is refused, the bound client is served",
                false,
                error,
            );
            return;
        }
    };

    let (code, text) = run_rogue(pipe.as_str());
    report.check(
        "a stranger that connects first is hung up on",
        code == Some(1) && text.contains("refused"),
        format!("exit {code:?}, said {text:?}"),
    );

    let served = match PipeClient::open_waiting(&pipe, Duration::from_secs(10)) {
        Ok(client) => {
            let trusted = client.server_is(&process)
                && client
                    .owner_sid()
                    .is_ok_and(|owner| is_trusted_owner(&owner));
            let hello = hello_over(&client);
            let bye = request_over(&client, 2, Request::Bye {});
            (trusted, hello, bye.is_ok())
        }
        Err(error) => (false, Err(format!("{error:?}")), false),
    };
    report.check(
        "the bound client is served by the helper that kept listening",
        served.0 && served.1.is_ok() && served.2,
        format!(
            "authenticated {}, hello {:?}, bye {}",
            served.0, served.1, served.2
        ),
    );
    let exit_code = ci::wait_exit_code(&process, Duration::from_secs(20));
    report.check(
        "that helper exits with code 0 after bye",
        exit_code == Some(0),
        format!("{exit_code:?}"),
    );
}

// ---------------------------------------------------------------------------------------------
// Negative cases

fn valid_args(me: &ProcessFacts) -> Vec<String> {
    PipeName::random().map_or_else(
        |_| Vec::new(),
        |pipe| {
            HelperArgs {
                pipe,
                ui_pid: me.identity.pid,
                ui_created_at_100ns: me.identity.created_at_100ns,
            }
            .render()
        },
    )
}

/// Starts `image` with `args`, waits for it to exit and returns its exit code.
fn exit_code_of(image: &Path, args: &[String]) -> Result<Option<u32>, String> {
    let process = ci::launch(image, args)?;
    Ok(ci::wait_exit_code(&process, Duration::from_secs(20)))
}

fn expect_exit(report: &mut Report, name: &str, image: &Path, args: &[String], want: u8) {
    match exit_code_of(image, args) {
        Ok(code) => report.check(
            name,
            code == Some(u32::from(want)),
            format!("exit code {code:?}, expected {want}"),
        ),
        Err(error) => report.check(name, false, error),
    }
}

fn negative_cases(report: &mut Report, image: &Path, me: &ProcessFacts) {
    let helper = image.with_file_name(HELPER_IMAGE_NAME);

    // Unknown and extra arguments: refused before any work.
    let good = valid_args(me);
    let with_extra = [good.clone(), vec!["--debug".to_owned()]].concat();
    let with_sid = [
        good.clone(),
        vec!["--client-sid".to_owned(), "S-1-5-32-544".to_owned()],
    ]
    .concat();
    let bad_cases: [(&str, Vec<String>); 5] = [
        ("no arguments", Vec::new()),
        ("serve without a pipe", vec!["serve".to_owned()]),
        ("one argument too many", with_extra),
        ("a client SID on the command line", with_sid),
        (
            "another mode word",
            vec!["debug".to_owned(), "--pipe".to_owned()],
        ),
    ];
    for (what, args) in bad_cases {
        expect_exit(
            report,
            &format!("arguments refused with the usage code: {what}"),
            &helper,
            &args,
            exit::USAGE,
        );
    }

    // The helper copied outside the install folder.
    let outside = std::env::temp_dir().join("volt-elevated-ci-outside");
    let copy = outside.join(HELPER_IMAGE_NAME);
    let copied = std::fs::create_dir_all(&outside).and_then(|()| std::fs::copy(&helper, &copy));
    match copied {
        Ok(_) => expect_exit(
            report,
            "a helper outside the install folder exits with the install code",
            &copy,
            &good,
            exit::INSTALL,
        ),
        Err(error) => report.check(
            "a helper outside the install folder exits with the install code",
            false,
            error.to_string(),
        ),
    }
    let _ = std::fs::remove_dir_all(&outside);

    // A wrong catalogue hash: the real client refuses the helper.
    let wrong = WinElevator::new(CatalogueHashes {
        tweaks: "0".repeat(64),
        cleaning: "0".repeat(64),
    });
    let result = wrong.hello();
    report.check(
        "a wrong catalogue hash gives Mismatch",
        matches!(result, Err(ElevationError::Mismatch(_))),
        format!("{result:?}"),
    );
    drop(wrong);

    writable_folder(report, image, &helper, &good);
}

/// A helper whose UI never connects ends by itself after its connection deadline (about 30 s)
/// instead of waiting for the idle limit. The harness stays alive, so this is the deadline and not
/// the end of the bound process.
fn abandoned_helper(report: &mut Report, image: &Path, me: &ProcessFacts) {
    let helper = image.with_file_name(HELPER_IMAGE_NAME);
    let started = Instant::now();
    let exit_code = exit_code_within(&helper, &valid_args(me), Duration::from_secs(60));
    let elapsed = started.elapsed();
    report.check(
        "a helper whose UI never connects ends by itself near the connection deadline",
        exit_code == Ok(Some(0))
            && elapsed >= Duration::from_secs(25)
            && elapsed < Duration::from_secs(50),
        format!("exit {exit_code:?} after {}", millis(elapsed)),
    );
}

/// Starts `image` with `args` and waits up to `limit` for its exit code.
fn exit_code_within(image: &Path, args: &[String], limit: Duration) -> Result<Option<u32>, String> {
    let process = ci::launch(image, args)?;
    Ok(ci::wait_exit_code(&process, limit))
}

fn icacls(args: &[&str]) -> Result<(), String> {
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_owned());
    let program = PathBuf::from(root).join("System32").join("icacls.exe");
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }
}

/// The install folder made writable for Users: both the availability check and the helper itself
/// refuse. The ACL is put back afterwards (the machine is throwaway, but the later steps and the
/// report should not depend on it).
fn writable_folder(report: &mut Report, image: &Path, helper: &Path, good: &[String]) {
    let Some(dir) = image.parent() else {
        report.check("a writable install folder is refused", false, "no folder");
        return;
    };
    let dir_text = dir.to_string_lossy().into_owned();
    let grant = format!("{USERS_SID}:(OI)(CI)M");
    if let Err(error) = icacls(&[&dir_text, "/grant", &grant]) {
        report.check("make the install folder writable for Users", false, error);
        return;
    }

    let status = status_for(image);
    report.check(
        "the availability check refuses a folder writable by Users",
        matches!(
            status,
            ElevationStatus::Unavailable {
                reason: UnavailableReason::InstallNotProtected { .. }
            }
        ),
        format!("{status:?}"),
    );
    let elevator = WinElevator::new(volt_helper::catalogue_hashes());
    let refused = elevator.hello();
    report.check(
        "the client starts no helper for it",
        matches!(refused, Err(ElevationError::Unavailable(_))),
        format!("{refused:?}"),
    );
    expect_exit(
        report,
        "the helper itself refuses to serve from it (install code)",
        helper,
        good,
        exit::INSTALL,
    );

    let restored = icacls(&[&dir_text, "/reset", "/T", "/C"]);
    let after = status_for(image);
    report.check(
        "resetting the folder's ACL makes the install available again",
        restored.is_ok() && after == ElevationStatus::Available,
        format!("{restored:?}, {after:?}"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(pairs: &'a [(&str, &str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_owned())
        }
    }

    #[test]
    fn only_a_hosted_runner_with_the_explicit_switch_may_run() {
        let all = [
            ("GITHUB_ACTIONS", "true"),
            ("RUNNER_ENVIRONMENT", "github-hosted"),
            ("VOLT_ELEVATED_CI", "1"),
        ];
        assert_eq!(check_environment(env(&all)), Ok(()));
        assert!(check_environment(env(&[])).is_err());
        // A developer machine that sets only the switch.
        assert!(check_environment(env(&[("VOLT_ELEVATED_CI", "1")])).is_err());
        // A self-hosted runner.
        let self_hosted = [
            ("GITHUB_ACTIONS", "true"),
            ("RUNNER_ENVIRONMENT", "self-hosted"),
            ("VOLT_ELEVATED_CI", "1"),
        ];
        assert!(check_environment(env(&self_hosted)).is_err());
        // The runner without the explicit switch of the workflow.
        let no_switch = [
            ("GITHUB_ACTIONS", "true"),
            ("RUNNER_ENVIRONMENT", "github-hosted"),
        ];
        assert!(check_environment(env(&no_switch)).is_err());
    }

    #[test]
    fn the_report_is_valid_text_in_both_forms() {
        let mut report = Report::default();
        report.check("a | b", true, "line\none \"quoted\"");
        report.fact("key", r"C:\path");
        assert!(report.markdown().contains(r"a \| b"));
        let json = report.json();
        assert!(json.contains(r#""detail": "line\none \"quoted\"""#));
        assert!(json.contains(r#""key": "C:\\path""#));
        assert!(report.all_passed());
        report.check("bad", false, "");
        assert!(!report.all_passed());
    }
}
