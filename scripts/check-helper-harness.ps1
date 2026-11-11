#!/usr/bin/env pwsh
# Proves what a helper binary holds of the CI harness: the accepting confirmer, the failpoint that
# ends the helper between the journal write and the change, and the test-only tweaks exist only in
# the harness build (the `elevated-ci` feature), never in the release helper.
#
#   check-helper-harness.ps1 -Binary <volt-helper.exe> -Expect absent    the release helper
#   check-helper-harness.ps1 -Binary <volt-helper.exe> -Expect present   the harness helper
#
# The second form is what makes the first meaningful: it shows the search finds the strings where
# they are. The strings are the marker the accepting confirmer carries, the id of the test tweak
# and the name of the throwaway registry key it changes; Rust keeps string constants as UTF-8, so
# a byte search is enough. Only reads the file. Never starts it.

param(
    [Parameter(Mandatory = $true)][string]$Binary,
    [Parameter(Mandatory = $true)][ValidateSet('absent', 'present')][string]$Expect
)

$ErrorActionPreference = 'Stop'

$markers = @(
    'volt-ci-accepting-confirmer',
    'ci-voltest-marker',
    'ci-voltest-open',
    'ci-voltest-crash',
    'volt-ci-failpoint',
    'SOFTWARE\VoltTest\Ci',
    'SOFTWARE\VoltTest\Crash'
)

$bytes = [System.IO.File]::ReadAllBytes($Binary)
# Latin-1 maps every byte to one character, so the search cannot be fooled by decoding.
$text = [System.Text.Encoding]::Latin1.GetString($bytes)

$failed = $false
foreach ($marker in $markers) {
    $found = $text.Contains($marker)
    $good = ($Expect -eq 'present') -eq $found
    $state = if ($found) { 'found' } else { 'not found' }
    Write-Host ("[{0}] '{1}' {2} in {3} ({4} bytes), expected {5}" -f $(if ($good) { 'PASS' } else { 'FAIL' }), $marker, $state, $Binary, $bytes.Length, $Expect)
    if (-not $good) { $failed = $true }
}
if ($failed) { exit 1 }
