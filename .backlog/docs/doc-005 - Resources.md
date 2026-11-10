---
id: doc-005
title: Resources
type: other
created_date: '2026-11-10T03:01:00Z'
updated_date: '2026-11-10T03:01:00Z'
---
References, prior art and research notes for the whole project. Add to this document whenever a
task turns up a source worth keeping. Mark anything unverified as such.

## Development machine (owner)

Windows 11 Pro build 26200, 32 GB RAM, RTX 5060 Ti 16 GB. Rust, Node 22, pnpm 10, MSVC Build
Tools, WebView2 and winget are installed. This is the primary test machine for memory, GPU and
VRAM features.

## Tooling snapshot (checked 2026-11-10)

| Tool | Version | Notes |
|---|---|---|
| Backlog.md | 1.55.3 | MIT. Dev dependency. MCP server exposes 24 tools: tasks, milestones, documents, Definition of Done defaults, workflow instructions. No decision tools |
| codegraph (`@colbymchenry/codegraph`) | 1.6.2 | MIT. Local SQLite index, native Windows, file watcher. MCP tools enabled through `CODEGRAPH_MCP_TOOLS`; telemetry disabled through `CODEGRAPH_TELEMETRY=0` |
| Biome | 2.6.6 | Also formats Markdown |
| lefthook | 2.2.2 | |
| commitlint | 21.2.4 | |
| Rust toolchain | 1.88.0 | Pinned in `rust-toolchain.toml`; raise it if a dependency needs a newer compiler |
| Tauri CLI / API (npm) | 2.12.4 | Not installed yet. crates.io also lists a 3.0 alpha: stay on 2.x |
| tauri-specta | 2.0.0-rc.26 | Still a release candidate; pin exactly if adopted |
| React / Vite / Tailwind / TypeScript / Vitest | 19.3 / 8.3 / 4.3 / 7.0 / 5.1 | npm latest; not installed yet |
| `windows` crate | 0.62.2 | |
| cargo-deny / cargo-nextest / typos / git-cliff | 0.20.2 / 0.9.151 / 1.52.1 / 2.14.2 | Not installed locally; CI runs cargo-deny and typos |

## Code-graph tool evaluation (2026-11-10)

| Tool | License | Storage | Verdict |
|---|---|---|---|
| codegraph 1.6.2 | MIT | SQLite in `.codegraph/` | **Adopted.** Small, local, auto-sync, callers/callees/impact tools |
| GitNexus 1.6.12 | PolyForm Noncommercial 1.0.0 | LadybugDB in `.gitnexus/` | Not adopted: noncommercial license, about 230 MB, needs OpenSSL 3 on PATH for full-text search on Windows, writes hooks and agent files into the repo. Has richer tools (`detect_changes`, `rename`) |
| codebase-memory-mcp 0.12.1 | MIT | SQLite in the user cache | Alternative if architecture queries and change detection become important |
| Serena 2.1.0 | GPL-3.0-or-later | LSP based | Optional refactoring helper; not needed now |
| CodeGraphContext 0.6.13 | MIT | Default backend is Unix-only | Not suitable on Windows without extra setup |

Not verified in practice: how well any of these resolve Rust and TSX symbols, and Tauri
`invoke()` links between TypeScript and Rust. Re-check once real code exists.

## Windows API references

Memory:

- `EmptyWorkingSet`: https://learn.microsoft.com/windows/win32/api/psapi/nf-psapi-emptyworkingset
- `SetSystemFileCacheSize`: https://learn.microsoft.com/windows/win32/api/memoryapi/nf-memoryapi-setsystemfilecachesize
- `GetPerformanceInfo`: https://learn.microsoft.com/windows/win32/api/psapi/nf-psapi-getperformanceinfo
- `NtSetSystemInformation` with `SystemMemoryListInformation`: undocumented. Commands used by
  existing tools: empty working sets, flush modified list, purge standby list, purge low-priority
  standby list. Requires `SeProfileSingleProcessPrivilege`; the file cache call requires
  `SeIncreaseQuotaPrivilege`. Verify constants against a maintained open-source implementation
  (for example Mem Reduct) before use.

Processes and power:

- `SetProcessInformation` with `ProcessPowerThrottling` (Efficiency mode / EcoQoS):
  https://learn.microsoft.com/windows/win32/api/processthreadsapi/nf-processthreadsapi-setprocessinformation
- Restart Manager, Toolhelp snapshots, `NtQuerySystemInformation` for process trees.

System:

- Task Scheduler COM API (hidden startup entries, start-with-Windows without UAC).
- Service Control Manager API (services).
- `SRSetRestorePoint` (restore points).
- USN change journal and `FSCTL_ENUM_USN_DATA` / MFT enumeration (fast disk scan).
- `SHEmptyRecycleBin`, `IFileOperation` with the recycle flag (safe deletion).
- NVML (NVIDIA) and D3DKMT / DXGI (GPU and VRAM statistics).

## Tauri references

- Docs: https://v2.tauri.app
- Updater plugin: https://v2.tauri.app/plugin/updater/ (`bundle.createUpdaterArtifacts` must be
  true; signing uses `TAURI_SIGNING_PRIVATE_KEY`)
- Windows code signing: https://tauri.app/distribute/sign/windows/ (EV and OV certificates now
  build SmartScreen reputation the same way; Azure Artifact Signing is an option)
- WebDriver testing: https://v2.tauri.app/develop/tests/webdriver/
- `create-tauri-app`: only `--template` is a documented flag (`react-ts` preset exists); check
  `--help` before relying on others
- GitHub Action: `tauri-apps/tauri-action@v1`; Rust cache: `Swatinem/rust-cache@v2`

## Prior art to study (behaviour and safety, not to copy code without checking licenses)

| Project | Why it matters |
|---|---|
| Microsoft PC Manager | The baseline for UX and scope |
| Mem Reduct (henrypp/memreduct) | Reference implementation of the memory-list operations |
| Intelligent Standby List Cleaner | Threshold-based standby purging (feature B6) |
| BleachBit and the Winapp2 rule set | Declarative cleaning rules and their safety conventions |
| WizTree, WinDirStat | MFT-based scanning and treemap UX |
| Chris Titus Tech WinUtil, Sophia Script, privacy.sexy | Catalogues of tweaks with their registry keys and reverts |
| O&O ShutUp10++ | Risk labelling of privacy tweaks |
| Bulk Crap Uninstaller | Leftover detection after uninstall |
| Process Lasso | Persistent process priority and affinity rules |

## Research not yet done

- Which "performance" tweaks have a measurable effect on Windows 11 24H2/25H2. Each tweak in the
  catalogue needs its own evidence note.
- WebView2 behaviour in an elevated process (drag and drop, user data folder location).
- Antivirus false-positive rates for elevated Tauri apps that read the MFT.
