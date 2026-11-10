#!/usr/bin/env pwsh
# Builds the release helper (volt-helper.exe): static C runtime, requireAdministrator manifest,
# DLL loading limited to System32, and none of the debug-only test modes.
#
# The helper loads only system DLLs, so it must not depend on the Visual C++ redistributable: the
# crate's build script refuses a release build without the static runtime. This script sets it up.
# It uses its own target folder so the flags do not invalidate the app's release cache.
#
# Building does not run the helper. Never start the release binary from a script or a test: it asks
# for administrator rights.

$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot -Parent)

$env:RUSTFLAGS = '-C target-feature=+crt-static'
cargo build -p volt-helper --release --locked --target-dir target/helper
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
Write-Host 'Built target/helper/release/volt-helper.exe' -ForegroundColor Green
