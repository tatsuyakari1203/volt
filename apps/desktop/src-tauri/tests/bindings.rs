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
fn generated_bindings_describe_the_autostart_commands() {
    let text = generated_bindings();
    assert!(text.contains("autostartEnabled"), "{text}");
    assert!(text.contains("setAutostart"), "{text}");
}

#[test]
fn generated_bindings_describe_boost_its_settings_and_the_tray_report_event() {
    let text = generated_bindings();
    for name in [
        "boostConfig",
        "setBoostConfig",
        "boostFinished",
        "export type BoostResult",
        "export type BoostConfigView",
        "movedToBin",
    ] {
        assert!(text.contains(name), "{name} missing in {text}");
    }
    // The tray no longer sends a request event: it runs Boost itself.
    assert!(!text.contains("trayBoostRequested"), "{text}");
}

#[test]
fn generated_bindings_describe_automation_its_bounds_and_the_run_event() {
    let text = generated_bindings();
    for name in [
        "automationStatus",
        "setAutomationConfig",
        "automationUpdated",
        "export type AutomationConfigView",
        "export type AutomationStatusView",
        "lastCleanSkip",
        "nextCleanRun",
        "standbyAboveMb",
    ] {
        assert!(text.contains(name), "{name} missing in {text}");
    }
}

#[test]
fn generated_bindings_describe_profiles_their_preview_and_the_update_event() {
    let text = generated_bindings();
    for name in [
        "listProfiles",
        "saveProfile",
        "deleteProfile",
        "previewProfileSwitch",
        "switchProfile",
        "undoProfileSwitch",
        "profilesUpdated",
        "export type ProfileView",
        "export type SwitchPreviewView",
        "export type SwitchResultView",
        "lastSwitch",
        "returnOnExit",
        "notAppliedByVolt",
    ] {
        assert!(text.contains(name), "{name} missing in {text}");
    }
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

#[test]
fn generated_bindings_describe_the_health_report_with_its_closed_fix_set() {
    let text = generated_bindings();
    assert!(text.contains("healthReport"), "{text}");
    assert!(text.contains("export type HealthView"), "{text}");
    assert!(text.contains("export type DeductionView"), "{text}");
    // The fix actions and the levels are closed sets the UI must handle one by one.
    for variant in [
        "\"openCleaner\"",
        "\"openMemory\"",
        "\"openStartup\"",
        "\"restartWindows\"",
        "\"good\"",
        "\"fair\"",
        "\"poor\"",
        "\"unknown\"",
    ] {
        assert!(text.contains(variant), "{variant} missing in {text}");
    }
}

#[test]
fn generated_bindings_describe_the_cleaner_commands_progress_event_and_results() {
    let text = generated_bindings();
    for name in [
        "listCleaningRules",
        "scanCleaning",
        "scanCleaningRules",
        "cancelCleaningScan",
        "cleanSelectedRules",
        "recycleBinStatus",
        "emptyRecycleBin",
        "cleanScanProgress",
        "export type CleanScanResult",
        "export type CleanRunResult",
    ] {
        assert!(text.contains(name), "{name} missing in {text}");
    }
    // Sizes cross as numbers, never as 64-bit integers.
    assert!(text.contains("bytes: number"), "{text}");
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

#[test]
fn generated_bindings_describe_the_memory_commands_and_their_report_types() {
    let text = generated_bindings();
    assert!(text.contains("memoryState"), "{text}");
    assert!(text.contains("cleanMemory"), "{text}");
    assert!(text.contains("MemoryCleanResult"), "{text}");
}

#[test]
fn generated_bindings_describe_the_large_files_commands_event_and_results() {
    let text = generated_bindings();
    for name in [
        "scanLargeFiles",
        "cancelLargeFilesScan",
        "trashLargeFiles",
        "revealLargeFile",
        "pickScanFolder",
        "largeFilesProgress",
        "LargeFilesResult",
        "LargeFilesTrashResult",
    ] {
        assert!(text.contains(name), "{name} is missing from the bindings");
    }
}
