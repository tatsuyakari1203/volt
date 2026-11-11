#!/usr/bin/env pwsh
# The installer checks of the elevated CI job (.github/workflows/elevated-helper.yml, job
# "Installer"). It INSTALLS and UNINSTALLS the real release installer, writes under Program Files
# and starts the real helper through the harness. It therefore refuses to run anywhere but the
# GitHub-hosted runner of that workflow (the same three switches the harness checks).
#
# Needs: scripts/build-installer.ps1 and the harness (cargo build -p volt-helper --features
# elevated-ci --bin volt-elevated-ci) already built.

param([Parameter(Mandatory)][string]$ReportDir)

$ErrorActionPreference = 'Stop'

if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted' -or $env:VOLT_ELEVATED_CI -ne '1') {
    Write-Error 'installer-checks.ps1 installs software and starts the elevated helper. It runs only in the elevated CI job on a GitHub-hosted runner.'
    exit 64
}

Set-Location (Split-Path $PSScriptRoot -Parent)
New-Item -ItemType Directory -Path $ReportDir -Force | Out-Null

$results = [System.Collections.Generic.List[object]]::new()
function Check([string]$Name, [bool]$Ok, [string]$Detail = '') {
    $tag = if ($Ok) { 'PASS' } else { 'FAIL' }
    Write-Host "[$tag] ${Name}: $Detail"
    $results.Add([pscustomobject]@{ Name = $Name; Ok = $Ok; Detail = $Detail })
}

$setup = Get-ChildItem 'target/release/bundle/nsis' -Filter '*-setup.exe' | Select-Object -First 1
$built = (Resolve-Path 'target/helper/release/volt-helper.exe').Path
$harness = (Resolve-Path 'target/debug/volt-elevated-ci.exe').Path
if (-not $setup) { throw 'No installer was built.' }
Write-Host "Installer: $($setup.FullName)"

function Invoke-Setup([string[]]$Arguments) {
    $process = Start-Process -FilePath $setup.FullName -ArgumentList $Arguments -Wait -PassThru
    return $process.ExitCode
}

# Starts the uninstaller. It copies itself to a temporary folder and ends, so wait for the files.
function Invoke-Uninstall([string]$Dir) {
    Start-Process -FilePath (Join-Path $Dir 'uninstall.exe') -ArgumentList '/S' | Out-Null
    $deadline = (Get-Date).AddSeconds(120)
    while ((Test-Path (Join-Path $Dir 'volt-desktop.exe')) -and (Get-Date) -lt $deadline) { Start-Sleep -Seconds 1 }
    # The folder itself goes last, once the temporary uninstaller has finished.
    while ((Test-Path $Dir) -and (Get-Date) -lt $deadline) { Start-Sleep -Seconds 1 }
}

function Install-Location([string]$Hive) {
    $key = "${Hive}:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Volt"
    if (-not (Test-Path $key)) { return $null }
    $value = (Get-ItemProperty $key).InstallLocation
    if ($value) { return $value.Trim('"') }
    return $null
}

# Principals that may own and change the install folder.
$trusted = @('S-1-5-18', 'S-1-5-32-544', 'S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464')
# Write data, append, write extended attributes, delete child, write attributes, delete, change
# permissions, take ownership, generic write, generic all.
$writeMask = 0x2 -bor 0x4 -bor 0x10 -bor 0x40 -bor 0x100 -bor 0x10000 -bor 0x40000 -bor 0x80000 -bor 0x40000000 -bor 0x10000000

function Get-Weaknesses([string]$Root) {
    $weak = @()
    $items = @(Get-Item -LiteralPath $Root -Force) + @(Get-ChildItem -LiteralPath $Root -Recurse -Force)
    foreach ($item in $items) {
        $acl = Get-Acl -LiteralPath $item.FullName
        $owner = $acl.GetOwner([System.Security.Principal.SecurityIdentifier]).Value
        if ($trusted -notcontains $owner) { $weak += "$($item.FullName) is owned by $owner" }
        foreach ($rule in $acl.Access) {
            if ($rule.AccessControlType -ne 'Allow') { continue }
            if (($rule.PropagationFlags -band [System.Security.AccessControl.PropagationFlags]::InheritOnly) -ne 0) { continue }
            try { $sid = $rule.IdentityReference.Translate([System.Security.Principal.SecurityIdentifier]).Value }
            catch { $sid = $rule.IdentityReference.Value }
            if ($trusted -contains $sid) { continue }
            if (([int]$rule.FileSystemRights -band $writeMask) -ne 0) {
                $weak += "$($item.FullName): $sid has $($rule.FileSystemRights)"
            }
        }
    }
    return $weak
}

function Run-Harness([string[]]$Arguments) {
    & $harness @Arguments 2>&1 | ForEach-Object { Write-Host $_ }
    return $LASTEXITCODE
}

$allUsers = Join-Path $env:ProgramFiles 'Volt'
$fake = $null

try {
    # ---------------------------------------------------------------------------------------
    # All users
    $code = Invoke-Setup @('/S', '/ALLUSERS')
    Check 'silent all-users install succeeds' ($code -eq 0) "exit code $code"

    $layout = @('volt-desktop.exe', 'volt-helper.exe', 'uninstall.exe') | ForEach-Object {
        [pscustomobject]@{ Name = $_; Exists = (Test-Path (Join-Path $allUsers $_)) }
    }
    Check 'the helper sits next to the UI in Program Files\Volt' (-not ($layout | Where-Object { -not $_.Exists })) `
        (($layout | ForEach-Object { "$($_.Name)=$($_.Exists)" }) -join ', ')
    Get-ChildItem $allUsers -Recurse -Force | ForEach-Object { Write-Host "  installed: $($_.FullName.Substring($allUsers.Length))" }
    Check 'Add or remove programs points at the folder (HKLM)' ((Install-Location 'HKLM') -eq $allUsers) "$(Install-Location 'HKLM')"

    $weak = Get-Weaknesses $allUsers
    Check 'no non-administrator can write to the install folder or anything in it' ($weak.Count -eq 0) ($weak -join '; ')
    icacls $allUsers | ForEach-Object { Write-Host "  icacls: $_" }
    icacls (Join-Path $allUsers 'volt-helper.exe') | ForEach-Object { Write-Host "  icacls: $_" }

    $code = Run-Harness @('installed', '--out', $ReportDir, '--ui', (Join-Path $allUsers 'volt-desktop.exe'), '--built', $built)
    Check 'the harness checks of the all-users install pass (availability, identical helper, helper started)' ($code -eq 0) "exit code $code, see installed-report.md"

    # An update while a helper runs: the installer ends it and replaces the file. The running
    # process is a copy of an unrelated system program under the helper's name, outside the folder.
    $fakeDir = Join-Path $env:RUNNER_TEMP 'volt-fake-helper'
    New-Item -ItemType Directory -Path $fakeDir -Force | Out-Null
    Copy-Item (Join-Path $env:SystemRoot 'System32\ping.exe') (Join-Path $fakeDir 'volt-helper.exe')
    $fake = Start-Process -FilePath (Join-Path $fakeDir 'volt-helper.exe') -ArgumentList '-t', '127.0.0.1' -WindowStyle Hidden -PassThru
    Start-Sleep -Seconds 1
    $running = -not $fake.HasExited
    $started = Get-Date
    $code = Invoke-Setup @('/S', '/ALLUSERS')
    $fake.Refresh()
    Check 'an update installs over a running helper and ends it' ($running -and $code -eq 0 -and $fake.HasExited) `
        "running before: $running, exit code $code, ended: $($fake.HasExited), took $([int]((Get-Date) - $started).TotalSeconds) s"
    $same = (Get-FileHash (Join-Path $allUsers 'volt-helper.exe')).Hash -eq (Get-FileHash $built).Hash
    Check 'the updated helper is the built one' $same ''

    Invoke-Uninstall $allUsers
    $left = @('volt-desktop.exe', 'volt-helper.exe', 'uninstall.exe') | Where-Object { Test-Path (Join-Path $allUsers $_) }
    Check 'the uninstaller removes the UI, the helper and itself' ($left.Count -eq 0) "left: $($left -join ', ')"
    Check 'the install folder is gone' (-not (Test-Path $allUsers)) ''
    # The uninstaller removes its registry entry as its last step, just after the folder, so give it a moment.
    $regDeadline = (Get-Date).AddSeconds(30)
    while ($null -ne (Install-Location 'HKLM') -and (Get-Date) -lt $regDeadline) { Start-Sleep -Seconds 1 }
    Check 'the all-users registration is gone' ($null -eq (Install-Location 'HKLM')) ''

    # ---------------------------------------------------------------------------------------
    # Just me
    $code = Invoke-Setup @('/S', '/CURRENTUSER')
    Check 'silent per-user install succeeds' ($code -eq 0) "exit code $code"
    $own = Install-Location 'HKCU'
    Check 'the per-user install is outside Program Files' ($own -and -not $own.StartsWith($env:ProgramFiles, [System.StringComparison]::OrdinalIgnoreCase)) "$own"
    if ($own) {
        Check 'the per-user install has no helper' (-not (Test-Path (Join-Path $own 'volt-helper.exe'))) ''
        $ui = Join-Path $own 'volt-desktop.exe'
        Check 'the per-user install has the UI' (Test-Path $ui) $ui
        $code = Run-Harness @('per-user', '--out', $ReportDir, '--ui', $ui)
        Check 'administrator features are reported unavailable for a per-user install' ($code -eq 0) "exit code $code, see per-user-report.md"
        # The same with a helper file put there by hand: the answer must not change.
        Copy-Item $built (Join-Path $own 'volt-helper.exe')
        $code = Run-Harness @('per-user', '--out', $ReportDir, '--ui', $ui)
        Check 'a helper file in a per-user folder changes nothing' ($code -eq 0) "exit code $code, see per-user-with-helper-report.md"
        Invoke-Uninstall $own
        $left = @('volt-desktop.exe', 'volt-helper.exe', 'uninstall.exe') | Where-Object { Test-Path (Join-Path $own $_) }
        Check 'the uninstaller removes the per-user files' ($left.Count -eq 0) "left: $($left -join ', ')"
    }
}
catch {
    Check 'the checks ran to the end' $false $_.Exception.Message
}
finally {
    if ($fake -and -not $fake.HasExited) { Stop-Process -Id $fake.Id -Force -ErrorAction SilentlyContinue }

    $md = "## Installer checks`n`n| Check | Result | Detail |`n| --- | --- | --- |`n"
    foreach ($r in $results) {
        $cell = ($r.Detail -replace '\|', '\|') -replace "[\r\n]+", ' '
        $md += "| $($r.Name) | $(if ($r.Ok) { 'pass' } else { 'FAIL' }) | $cell |`n"
    }
    Set-Content -Path (Join-Path $ReportDir 'installer-checks.md') -Value $md -Encoding utf8
}

if ($results | Where-Object { -not $_.Ok }) { exit 1 }
exit 0
