//! Build script of the helper.
//!
//! One switch decides what kind of binary this is: the release profile. The build script turns it
//! into the cfg `helper_release`, which gates both halves, so they cannot disagree.
//!
//! - In dev and test builds the binary has NO administrator manifest, so it starts without a prompt
//!   and the tests can run it unelevated, and it carries the test modes (see `src/debug.rs`).
//! - In release builds (`helper_release`) the binary gets the `requireAdministrator` manifest and
//!   the linker flag that limits DLL loading to System32, and it has no test modes at all. The
//!   helper must also link the C runtime statically, because it may load only system DLLs; the
//!   build refuses to produce a release helper that does not.
//!
//! So a binary that asks for administrator rights can never be the one with test modes in it.
//! Build the release helper with `scripts/build-helper.ps1`.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=volt-helper.manifest");
    println!("cargo:rustc-check-cfg=cfg(helper_release)");

    let windows = std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows");
    let msvc = std::env::var("CARGO_CFG_TARGET_ENV").is_ok_and(|env| env == "msvc");
    let release = std::env::var("PROFILE").is_ok_and(|profile| profile == "release");
    if release {
        println!("cargo:rustc-cfg=helper_release");
    }
    if !windows || !msvc || !release {
        return;
    }

    let crt_static = std::env::var("CARGO_CFG_TARGET_FEATURE")
        .is_ok_and(|features| features.split(',').any(|feature| feature == "crt-static"));
    assert!(
        crt_static,
        "the release helper must link the C runtime statically (it loads only system DLLs). \
         Build it with scripts/build-helper.ps1, which sets RUSTFLAGS='-C target-feature=+crt-static'."
    );

    let Ok(dir) = std::env::var("CARGO_MANIFEST_DIR") else {
        return;
    };
    println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
    // The linker would add its own `asInvoker` snippet next to ours and the two would conflict.
    println!("cargo:rustc-link-arg-bins=/MANIFESTUAC:NO");
    println!("cargo:rustc-link-arg-bins=/MANIFESTINPUT:{dir}\\volt-helper.manifest");
    // Load only from System32 whatever the code (or a planted DLL) asks for: 0x800 is
    // LOAD_LIBRARY_SEARCH_SYSTEM32.
    println!("cargo:rustc-link-arg-bins=/DEPENDENTLOADFLAG:0x800");
}
