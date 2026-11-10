<#
.SYNOPSIS
  Lists the remote endpoints of Volt's process tree (volt-desktop.exe and its msedgewebview2.exe
  children). Read-only observation: it starts the app, waits, samples, stops only what it started.

.EXAMPLE
  pwsh -NoProfile -File scripts/observe-webview2.ps1 -Seconds 90
  pwsh -NoProfile -File scripts/observe-webview2.ps1 -Label baseline -BrowserArgs '--disable-background-networking'

.NOTES
  -BrowserArgs is put in WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS of the started process only; nothing
  on the machine is changed. Refuses to run while another volt-desktop.exe is running (single
  instance). Needs a built app: pnpm tauri build --no-bundle.
#>
param(
  [string]$Exe = (Join-Path $PSScriptRoot '..\target\release\volt-desktop.exe'),
  [int]$Seconds = 70,
  [string]$BrowserArgs = '',
  [string]$Label = 'run'
)
$ErrorActionPreference = 'Stop'
$Exe = (Resolve-Path $Exe).Path
if (Get-Process -Name volt-desktop -ErrorAction SilentlyContinue) {
  throw 'volt-desktop.exe is already running; close it first (single-instance app).'
}

function Get-Tree([int]$RootPid) {
  $all = Get-CimInstance Win32_Process -Property ProcessId, ParentProcessId, Name, CommandLine
  $ids = [System.Collections.Generic.HashSet[int]]::new()
  [void]$ids.Add($RootPid)
  do {
    $added = $false
    foreach ($p in $all) {
      if ($ids.Contains([int]$p.ParentProcessId) -and $ids.Add([int]$p.ProcessId)) { $added = $true }
    }
  } while ($added)
  $all | Where-Object { $ids.Contains([int]$_.ProcessId) }
}

function Get-Kind($p) {
  if ($p.Name -eq 'volt-desktop.exe') { return 'volt-desktop' }
  if ($p.CommandLine -match '--type=([\w-]+)') {
    $t = $Matches[1]
    if ($t -eq 'utility' -and $p.CommandLine -match '--utility-sub-type=([\w\.]+)') { return "webview2-utility:$($Matches[1])" }
    return "webview2-$t"
  }
  return 'webview2-browser'
}

$old = $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS
if ($BrowserArgs) { $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = $BrowserArgs }
$proc = Start-Process -FilePath $Exe -PassThru
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = $old
$seen = @{}
$maxProcs = 0
$flags = $null
try {
  $start = Get-Date
  while (((Get-Date) - $start).TotalSeconds -lt $Seconds) {
    if ($proc.HasExited) { Write-Warning 'volt-desktop.exe exited early'; break }
    $tree = @(Get-Tree $proc.Id)
    $maxProcs = [Math]::Max($maxProcs, $tree.Count)
    if (-not $flags) { $b = $tree | Where-Object { $_.Name -eq "msedgewebview2.exe" -and $_.CommandLine -notmatch "--type=" } | Select-Object -First 1; if ($b) { $flags = ($b.CommandLine -split " (?=--)" | Where-Object { $_ -match "^--(disable|no-pings|autoplay)" }) -join " " } }
    $byPid = @{}
    foreach ($p in $tree) { $byPid[[int]$p.ProcessId] = $p }
    $t = [int]((Get-Date) - $start).TotalSeconds
    $conns = @(Get-NetTCPConnection -ErrorAction SilentlyContinue | Where-Object { $byPid.ContainsKey([int]$_.OwningProcess) }) +
             @(Get-NetUDPEndpoint -ErrorAction SilentlyContinue | Where-Object { $byPid.ContainsKey([int]$_.OwningProcess) })
    foreach ($c in $conns) {
      $remote = if ($c.PSObject.Properties['RemoteAddress']) { "$($c.RemoteAddress):$($c.RemotePort)" } else { "udp-local:$($c.LocalPort)" }
      $proto = if ($c.PSObject.Properties['RemoteAddress']) { 'tcp' } else { 'udp' }
      # Listening sockets have no remote side; they are not connections.
      if ($proto -eq 'tcp' -and [int]$c.RemotePort -eq 0) { continue }
      $kind = Get-Kind $byPid[[int]$c.OwningProcess]
      $key = "$kind|$proto|$remote"
      if (-not $seen.ContainsKey($key)) { $seen[$key] = [pscustomobject]@{ Process = $kind; Proto = $proto; Remote = $remote; First = $t; Last = $t } }
      else { $seen[$key].Last = $t }
    }
    Start-Sleep -Seconds 1
  }
} finally {
  # Stop only what this script started: the root and its descendants.
  if ($proc -and -not $proc.HasExited) {
    $tree = @(Get-Tree $proc.Id)
    foreach ($p in ($tree | Sort-Object ProcessId -Descending)) { Stop-Process -Id $p.ProcessId -Force -ErrorAction SilentlyContinue }
  }
}

Write-Output "label=$Label seconds=$Seconds args='$BrowserArgs' max_processes=$maxProcs"
Write-Output "browser flags: $flags"
$rows = $seen.Values | Where-Object { $_.Remote -notmatch '^(127\.|::1|\[::1\])' } | Sort-Object Process, First
if (-not $rows) { Write-Output 'no remote endpoints observed' }
else { $rows | Format-Table Process, Proto, Remote, First, Last -AutoSize | Out-String -Width 200 | Write-Output }
$volt = $rows | Where-Object { $_.Process -eq 'volt-desktop' }
Write-Output ("volt-desktop own endpoints: {0}" -f @($volt).Count)
