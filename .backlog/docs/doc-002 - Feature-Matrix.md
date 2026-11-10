---
id: doc-002
title: Feature Matrix
type: specification
created_date: '2026-11-10T03:01:00Z'
updated_date: '2026-11-10T03:01:00Z'
---
The full scope of Volt compared with Microsoft PC Manager. Row IDs (A1, B2, ...) are referenced
from tasks. Priority maps to a milestone: **P0** = M1 (MVP v0.1), **P1** = M2 (v0.2), **P2** = M3
(v0.3), **P3** = M4 (v1.0).

The "PC Manager" column was written from memory of the current release and has not been checked
feature by feature against an installed copy. Treat it as a guide, not a claim.

Keep this document current: when a feature ships, is cut or changes priority, update its row.

## A. Dashboard and Boost

| ID | Feature | PC Manager | Volt | Priority | Technique |
|---|---|---|---|---|---|
| A1 | One-click Boost (memory and temp files) | Yes | Yes, with a before/after report | P0 | Core pipeline |
| A2 | System health score | Basic | 0-100 score with reasons and a fix button per item | P0 | Rule engine |
| A3 | Live metrics: CPU, RAM, GPU, VRAM, disk | RAM only | Full | P0 | PDH, NVML, DXGI |
| A4 | Tray flyout | Yes (toolbar) | Yes | P1 | Tauri tray |
| A5 | Always-on-top floating widget | Yes | Yes | P2 | Secondary window |
| A6 | Automatic smart boost | Yes | Yes, user-defined thresholds | P2 | Background agent |

## B. Memory

| ID | Feature | PC Manager | Volt | Priority | Technique |
|---|---|---|---|---|---|
| B1 | Trim working sets of all processes | Yes | Yes | P0 | `EmptyWorkingSet` |
| B2 | Purge standby list (including priority 0) | No | Yes | P0 | `NtSetSystemInformation(SystemMemoryListInformation)` |
| B3 | Flush modified page list | No | Yes | P0 | Same NT API |
| B4 | Trim system file cache | No | Yes | P0 | `SetSystemFileCacheSize` |
| B5 | Combine page list, flush registry cache | No | Yes | P1 | NT API |
| B6 | Auto-clean on thresholds (standby above X and free below Y) | No | Yes | P2 | Background agent |
| B7 | Process exclusion list | No | Yes | P1 | Settings |
| B8 | RAM composition chart (in use, standby, modified, free) | No | Yes | P1 | `GetPerformanceInfo` |
| B9 | VRAM per process | No | Yes | P2 | NVML, D3DKMT |
| B10 | Memory compression toggle, pagefile view | No | Yes | P2 | MMAgent |

## C. Cleaning

| ID | Feature | PC Manager | Volt | Priority | Technique |
|---|---|---|---|---|---|
| C1 | Temp files, WER, crash dumps, logs, thumbnail cache | Yes | Yes | P0 | Declarative rules |
| C2 | Windows Update cache, Delivery Optimization | Yes | Yes | P0 | Stop service, then clean |
| C3 | Recycle Bin | Yes | Yes | P0 | Shell API |
| C4 | Browser caches (Chrome, Edge, Firefox, Brave) | Yes | Yes, per profile | P0 | Rules |
| C5 | Shader caches (DirectX, NVIDIA, AMD) | No | Yes | P1 | Rules |
| C6 | Chat app caches (Zalo, Discord, Telegram, Teams) | No | Yes | P1 | Rules |
| C7 | Developer caches (npm, pnpm, pip, cargo, NuGet, Gradle, Docker) | No | Yes | P1 | Rules plus native commands |
| C8 | AI caches (Hugging Face, ComfyUI temp and output, torch hub) | No | Yes | P1 | Rules |
| C9 | Component store (WinSxS), Windows.old | Partly | Yes | P1 | DISM |
| C10 | Disable hibernation to reclaim hiberfil.sys | No | Yes | P1 | `powercfg` |
| C11 | Disk analyzer: treemap with fast MFT scan | No | Yes | P1 | USN journal / MFT |
| C12 | Large files finder | Yes | Yes | P1 | From the MFT index |
| C13 | Duplicate files finder | Yes | Yes, three-stage hashing | P2 | Size, partial hash, xxHash |
| C14 | Empty folders, broken shortcuts | No | Yes | P2 | Scan |
| C15 | User-defined cleaning rules | No | Yes | P2 | TOML |
| C16 | Scheduled cleaning | Storage Sense | Yes | P2 | Background agent |

## D. Startup, processes and services

| ID | Feature | PC Manager | Volt | Priority | Technique |
|---|---|---|---|---|---|
| D1 | Startup apps (Run keys, Startup folder) | Yes | Yes | P0 | Registry |
| D2 | Hidden startup (scheduled tasks, services, shell extensions) | No | Yes | P1 | Task Scheduler COM, SCM |
| D3 | Delayed startup | No | Yes | P2 | Task with delay |
| D4 | Process list and end task | Yes | Yes, tree view grouped by app | P0 | Toolhelp, NtQuery |
| D5 | Priority, affinity, Efficiency mode, persisted | No | Yes | P1 | `SetProcessInformation` (EcoQoS) |
| D6 | Services manager with safe presets | No | Yes | P2 | SCM |
| D7 | Resource hog detection | No | Yes | P2 | Background agent |

## E. Tweaks (declarative engine, always reversible)

| ID | Group | Examples | PC Manager | Priority |
|---|---|---|---|---|
| E1 | Performance | Ultimate power plan, visual effects, background apps, SysMain, Fast Startup, Win32PrioritySeparation | No | P0 |
| E2 | Gaming | Game Mode, HAGS, fullscreen optimizations, MMCSS, per-app GPU preference, timer resolution | No | P1 |
| E3 | Privacy | Telemetry, advertising ID, activity history, Copilot and Recall, Bing in Start, suggestions and ads | No | P0 |
| E4 | Appearance | Classic context menu, taskbar alignment, Widgets off, file extensions, Explorer options | No | P0 |
| E5 | Network | DNS switcher, flush DNS, Winsock reset, TCP autotuning, Nagle, network throttling | No | P1 |
| E6 | Windows Update | Pause, defer, block driver updates, active hours | Partly | P1 |
| E7 | Security (Advanced label) | VBS / Memory Integrity, Spectre mitigations | No | P3 |
| E8 | Debloat | Remove preinstalled UWP apps, OneDrive, Edge background tasks | No | P2 |
| E9 | Context menu manager | Enable or disable individual right-click entries | No | P3 |
| E10 | Profiles | Gaming, Work, AI workload, Battery saver | No | P2 |
| E11 | Import and export of tweak configuration | - | No | P2 |
| E12 | Drift detection | Detect tweaks reset by Windows Update | No | P2 |
| E13 | Restore point before risky tweaks | - | No | P0 |

## F. Apps

| ID | Feature | PC Manager | Volt | Priority | Technique |
|---|---|---|---|---|---|
| F1 | Uninstall apps (Win32 and UWP), batch uninstall | Yes | Yes | P1 | Uninstall registry keys, Appx |
| F2 | Leftover scan after uninstall | No | Yes | P2 | Heuristics |
| F3 | Update all apps | No | Yes | P1 | winget |
| F4 | Restore default apps, taskbar repair | Yes | Yes | P3 | - |

## G. Health and repair

| ID | Feature | PC Manager | Volt | Priority | Technique |
|---|---|---|---|---|---|
| G1 | Overall health check | Yes | Yes | P0 | Rules |
| G2 | SFC, DISM and chkdsk with a UI | No | Yes | P3 | CLI wrappers |
| G3 | Disk SMART, temperature, TRIM / optimize | No | Yes | P2 | WMI, storage IOCTL |
| G4 | Battery report | No | Yes | P3 | `powercfg` |
| G5 | Network repair, Windows Update repair | Partly | Yes | P3 | - |

## H. Experience and platform

| ID | Feature | PC Manager | Volt | Priority |
|---|---|---|---|---|
| H1 | Dark and light themes, Mica, smooth motion | Yes | Yes | P0 |
| H2 | Activity log and Undo page | No | Yes | P0 |
| H3 | Command palette (Ctrl+K), global Boost hotkey | No | Yes | P2 |
| H4 | Vietnamese and English | Yes | Yes | P1 |
| H5 | CLI (`volt boost`, `volt clean --dry-run`) | No | Yes | P2 |
| H6 | No telemetry, no ads, works offline | No | Yes | P0 |
| H7 | Installer and auto-update | Yes | Yes | P3 |
| H8 | Block pop-up ads from apps | Yes | Yes | P3 |
| H9 | Toolbox (screenshot, translate, Circle to Act, AI tools, Agents) | Yes | Not planned: Windows already has these | - |
