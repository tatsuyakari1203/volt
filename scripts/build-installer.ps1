#!/usr/bin/env pwsh
# Builds the Windows installer (NSIS) with the release helper inside it.
#
# 1. scripts/build-helper.ps1 builds volt-helper.exe (static runtime, administrator manifest).
#    It runs in a child process so its RUSTFLAGS do not leak into the app build below.
# 2. tauri build bundles the app with src-tauri/tauri.helper.conf.json merged over the base
#    configuration. That overlay adds the helper as a resource named volt-helper.exe, which the
#    installer puts next to volt-desktop.exe. The base configuration has no such resource, so a
#    plain debug build and `pnpm tauri dev` need no helper.
#
# Building does not run the installer or the helper. The installer asks for administrator rights
# when "all users" is chosen; do not run it from a script on a developer machine.

$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot -Parent)

pwsh -NoProfile -File scripts/build-helper.ps1
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

pnpm --filter '@volt/desktop' exec tauri build --bundles nsis --config src-tauri/tauri.helper.conf.json
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
