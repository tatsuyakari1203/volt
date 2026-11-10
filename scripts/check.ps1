#!/usr/bin/env pwsh
# Runs every quality gate that CI runs. Usage: pnpm check
# Optional tools (cargo-deny, typos) run only when installed; CI always runs them.

$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot -Parent)

$steps = [ordered]@{
    'rustfmt'      = { cargo fmt --all -- --check }
    'clippy'       = { cargo clippy --workspace --all-targets --locked -- -D warnings }
    'tests'        = { cargo test --workspace --locked }
    'architecture' = { node scripts/check-arch.mjs }
    'biome'        = { pnpm exec biome check . }
    'typecheck'    = { pnpm --filter '@volt/desktop' typecheck }
    'vitest'       = { pnpm --filter '@volt/desktop' test }
    'ui-boundaries' = { pnpm --filter '@volt/desktop' boundaries }
}

if (Get-Command cargo-deny -ErrorAction SilentlyContinue) {
    $steps['cargo-deny'] = { cargo deny check }
}
if (Get-Command typos -ErrorAction SilentlyContinue) {
    $steps['typos'] = { typos }
}

$failed = @()
foreach ($name in $steps.Keys) {
    Write-Host "`n==> $name" -ForegroundColor Cyan
    & $steps[$name]
    if ($LASTEXITCODE -ne 0) {
        $failed += $name
        Write-Host "FAILED: $name" -ForegroundColor Red
    }
}

Write-Host ''
if ($failed.Count -gt 0) {
    Write-Host "Check failed: $($failed -join ', ')" -ForegroundColor Red
    exit 1
}
Write-Host "All $($steps.Count) checks passed." -ForegroundColor Green
