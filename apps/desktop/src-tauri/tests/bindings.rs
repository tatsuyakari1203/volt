//! The generated TypeScript bindings against the Rust definitions.
//!
//! This is an integration test (not a unit test) because it links the Tauri runtime, and only
//! integration-test executables get the Windows manifest from `build.rs`.

use std::path::Path;

/// Where the generated bindings are committed.
const COMMITTED_BINDINGS: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../src/shared/ipc/bindings.ts");

/// Set to `1` to rewrite the committed bindings instead of failing on drift.
const UPDATE_ENV: &str = "VOLT_UPDATE_BINDINGS";

/// Exports the bindings to a scratch file and returns their text.
fn generated_bindings() -> String {
    let dir = tempfile::tempdir().expect("create scratch dir");
    let file = dir.path().join("bindings.ts");
    volt_desktop_lib::export_bindings(&file).expect("bindings export");
    std::fs::read_to_string(&file).expect("read generated bindings")
}

#[test]
fn generated_bindings_describe_the_sample_command_event_and_error() {
    let text = generated_bindings();
    assert!(text.contains("appInfo"), "{text}");
    assert!(text.contains("appReady"), "{text}");
    assert!(text.contains("IpcError"), "{text}");
}

#[test]
fn generated_bindings_describe_the_autostart_commands_and_the_tray_boost_event() {
    let text = generated_bindings();
    assert!(text.contains("autostartEnabled"), "{text}");
    assert!(text.contains("setAutostart"), "{text}");
    assert!(text.contains("trayBoostRequested"), "{text}");
}

#[test]
fn generated_bindings_describe_live_metrics_with_optional_gpu_values() {
    let text = generated_bindings();
    assert!(text.contains("latestMetrics"), "{text}");
    assert!(text.contains("metricsUpdated"), "{text}");
    // A machine without a GPU is a normal case, so the GPU and its numbers are nullable.
    assert!(text.contains("gpu: GpuView | null"), "{text}");
    assert!(text.contains("utilisationPercent: number | null"), "{text}");
    assert!(text.contains("vramTotalBytes: number | null"), "{text}");
}

/// The drift gate: the committed bindings must equal what the Rust definitions generate.
/// `scripts/bindings.ps1` (and so `pnpm check`) runs this test; `pnpm bindings` rewrites the
/// committed file by setting `VOLT_UPDATE_BINDINGS=1`.
#[test]
fn committed_bindings_match_the_rust_definitions() {
    let generated = generated_bindings();
    if std::env::var_os(UPDATE_ENV).is_some_and(|value| value == "1") {
        std::fs::write(Path::new(COMMITTED_BINDINGS), &generated)
            .expect("rewrite committed bindings");
        return;
    }
    let committed = std::fs::read_to_string(COMMITTED_BINDINGS)
        .expect("committed bindings are missing: run `pnpm bindings`");
    // assert!, not assert_eq!: a failing assert_eq! would print both whole files.
    assert!(
        committed.replace("\r\n", "\n") == generated,
        "apps/desktop/src/shared/ipc/bindings.ts is out of date: run `pnpm bindings`"
    );
}
