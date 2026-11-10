---
id: decision-001
title: Tauri 2 with a Rust core and React UI
date: '2026-11-10 02:43'
status: accepted
---
## Context

Volt needs deep access to Windows (NT memory APIs, registry, services, the MFT), a small idle footprint, and a UI that is quick to build and easy to make attractive. The options considered were Tauri 2 with Rust and a web UI, and WinUI 3 with C#. The development machine already has Rust, Node, pnpm, MSVC Build Tools and WebView2; it has no .NET SDK.

## Decision

Build Volt with Tauri 2: a Rust core and adapters, and a React + TypeScript + Tailwind UI rendered in WebView2. Stay on Tauri 2.x stable; do not adopt the 3.0 alpha.

## Consequences

- Windows APIs are called directly through the `windows` crate, without P/Invoke.
- The installer is small (roughly 10 MB) and the Rust side can idle at a few megabytes when the WebView is destroyed.
- The UI imitates the Windows 11 look rather than using native controls; Mica and Fluent details need custom work.
- WebView2 must be present (it ships with Windows 11) and its behaviour in an elevated process needs to be checked.
- UI work can be previewed in a browser, which speeds up iteration.
