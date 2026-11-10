---
id: doc-001
title: Product Plan
type: specification
created_date: '2026-11-10T03:01:00Z'
updated_date: '2026-11-10T03:01:00Z'
---
Working name: **Volt**. An open-source Windows optimizer meant to replace Microsoft PC Manager for
people who want more depth and full control: memory cleaning, junk removal, startup and process
control, and a catalogue of system tweaks that can always be undone.

## Who it is for

- Power users and gamers on Windows 10 and 11 who currently combine several tools (PC Manager, a
  RAM cleaner, a debloat script, a disk analyzer).
- Developers and people running local AI workloads, whose machines fill up with package caches,
  model caches and shader caches that generic cleaners do not know about.

## Principles

1. **Everything is reversible.** Each change has a read-only detect step, an apply step and a
   revert step. The previous value is written to an undo journal before anything changes.
2. **Preview before action.** Cleaning always shows what will be removed and how much space it
   frees. Deletion goes to the Recycle Bin when possible.
3. **Honest labels.** Every tweak shows a risk level (Safe, Caution, Advanced) and whether its
   benefit is measured or anecdotal. Tweaks that cannot be explained are not shipped.
4. **Quiet.** No telemetry, no ads, works offline, small idle footprint (the UI is destroyed when
   the window is hidden; only a small tray process stays).
5. **Simple first.** One-click actions and plain language by default; an advanced mode exposes the
   exact registry keys, services and paths.

## Non-goals

- A toolbox of unrelated utilities (screenshot, translator, currency converter) or AI features.
- "Registry cleaners", driver updaters and other features with poor evidence and real risk.
- Platforms other than Windows.

## User experience

- **Layout:** a sidebar with six sections: Home, Clean, Memory, Tweaks, Apps and Startup, Health.
  Settings and Undo sit at the bottom.
- **Home:** a health score ring, one large Boost button, four live metric cards and a short list
  of recommendations.
- **Two modes:** Simple (grouped switches, Safe tweaks only) and Advanced (every tweak, with the
  keys and services it changes).
- **Every action** has a preview, a result stated as an outcome ("freed 3.2 GB") and an Undo
  button on the confirmation toast.
- **Window:** compact portrait by default (about 460 x 700, like PC Manager), resizable for the
  disk treemap and the process table.

## Milestones

| Milestone | Scope | Exit criteria |
|---|---|---|
| M0 Foundation | Tauri shell, design system and UI mock, typed IPC, core domain and ports, Windows adapters, undo journal, tweak engine, tray lifecycle, logging | The app starts, one sample tweak can be applied and undone end to end |
| M1 MVP v0.1 | Every P0 row of the Feature Matrix: Home, Boost, deep memory cleaning, core junk cleaning, startup, processes, about 30 tweaks, Undo page | Usable every day as a PC Manager replacement |
| M2 v0.2 Deep clean and apps | P1 rows: extended cleaning rules (dev, AI, chat, shaders), disk analyzer, large files, uninstaller, winget updates, hidden startup, gaming and network tweaks, localisation | Clearly ahead of PC Manager on cleaning |
| M3 v0.3 Automation | P2 rows: schedules and thresholds, profiles, duplicates, services, disk health, widget, CLI, debloat | Runs unattended in the background |
| M4 v1.0 Release | P3 rows: repair toolkit, context menu manager, installer, auto-update, signing | Public release |

Priorities map to milestones: P0 = M1, P1 = M2, P2 = M3, P3 = M4. The approach is to ship the MVP
early and improve from real logs rather than soak-test for long periods.

## Open questions

- **License** (see the license decision): recommended GPL-3.0-or-later, not yet confirmed.
- **Code signing:** an elevated, unsigned app triggers SmartScreen. Fine for personal use; a
  public release needs a certificate (recurring cost) or Azure Artifact Signing.
- **Name:** "Volt" is a working name and collides with existing products (for example the Volta
  tool manager). Decide before the first public release.

## Honest limits

- Memory cleaning helps in specific situations (a full standby list causing stutter in games,
  loading large AI models). It is not a general speed-up, and the UI should say so.
- The memory-list operations use undocumented NT APIs. They have been stable for many Windows
  releases but must be re-verified on each new build.
- Reading the MFT and running elevated make antivirus false positives more likely.
