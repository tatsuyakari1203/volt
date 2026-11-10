#!/usr/bin/env pwsh
# Generated TypeScript bindings for the Tauri commands and events.
#   pnpm bindings          regenerate apps/desktop/src/shared/ipc/bindings.ts from the Rust code
#   pnpm bindings:check    regenerate in memory and fail if the committed file differs (CI gate)
param([switch]$Check)

$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot -Parent)

$test = 'committed_bindings_match_the_rust_definitions'
if ($Check) {
    Remove-Item Env:VOLT_UPDATE_BINDINGS -ErrorAction SilentlyContinue
} else {
    $env:VOLT_UPDATE_BINDINGS = '1'
}
cargo test -p volt-desktop --test bindings --locked $test
exit $LASTEXITCODE
