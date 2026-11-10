---
id: decision-003
title: Single elevated process with an on-demand window
date: '2026-11-10T03:01:00Z'
status: proposed
---
## Context

Most Volt operations need administrator rights. The choices are (a) one elevated process that hosts the tray, the core and the Tauri window, or (b) an unelevated UI talking to an elevated helper over a named pipe. Option (a) is one binary and no IPC protocol; option (b) keeps the WebView unprivileged but adds a second binary, a protocol and an authentication problem on the pipe.

## Decision

Proposed: option (a). The process starts elevated (through a highest-privileges scheduled task when started with Windows, so there is no UAC prompt at login), and the window and its WebView are created on demand and destroyed when hidden. To be confirmed or replaced by the elevation spike task in M0.

## Consequences

- Simplest path to an MVP.
- The WebView runs elevated, so the UI must load only bundled content and the Tauri capability set must be minimal.
- Drag and drop from unelevated apps into the window will not work.
- If the spike shows unacceptable behaviour, switching to option (b) means adding an IPC crate and a rule for it.
