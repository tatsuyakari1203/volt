fn main() {
    tauri_build::build();
    embed_test_manifest();
}

/// Test executables of this crate link the Tauri runtime, which needs Common Controls v6 on
/// Windows. `tauri-build` embeds that manifest in the app binary only, so without this the test
/// binary dies at startup with `STATUS_ENTRYPOINT_NOT_FOUND`.
fn embed_test_manifest() {
    let msvc = std::env::var("CARGO_CFG_TARGET_ENV").is_ok_and(|env| env == "msvc");
    if !msvc {
        return;
    }
    let Ok(dir) = std::env::var("CARGO_MANIFEST_DIR") else {
        return;
    };
    println!("cargo:rerun-if-changed=windows-test.manifest");
    println!("cargo:rustc-link-arg-tests=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg-tests=/MANIFESTINPUT:{dir}\\windows-test.manifest");
}
