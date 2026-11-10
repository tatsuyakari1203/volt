# Privacy statement

Volt works offline and sends nothing. This page says exactly what that means, what is checked
automatically, and what is outside Volt's control: the Microsoft WebView2 runtime that draws the
window makes connections of its own (see the run-time verification at the end).

## What Volt does

- Reads system state (memory, disk, startup entries, registry values) and changes it only when you
  confirm a change. Everything it reads and every change it makes stays on your machine.
- Keeps its own data in two places on your machine: the undo journal and settings (a SQLite
  database) and a small, size-bounded diagnostic log (`volt.log`, at most three files of 512 KiB).
  The log names targets and outcomes only; see "Redaction rule" in
  [architecture.md](architecture.md#redaction-rule).
- Opens its window through the Tauri shell and the WebView2 runtime. The page is served from
  inside the app, not from a website.

## What Volt never sends

- No telemetry, analytics, crash reports, usage statistics or identifiers, to anyone.
- No update checks. There is no updater in the app today.
- No ads, no accounts, no sign-in and no licence check.
- No file names, registry values, process lists or logs. Nothing leaves the machine because the
  app contains no code that sends anything.

If a feature ever needs the network (for example an optional update check or winget), it must say
so in the UI, be off or ask first, and be added to this page in the same change.

## What is checked, not just promised

`pnpm check` and CI run `scripts/check-privacy.mjs`, which fails when any of the following changes
without this page changing in the same commit:

1. A Tauri capability grants a network-capable permission (http, websocket, updater, upload,
   opener, shell, creating windows or webviews, or `remote` access), or `tauri.conf.json` adds a
   remote window, remote IPC, an updater or a network plugin.
2. The shipped dependency graph (Windows target, normal and build edges) gains a network-capable
   crate (HTTP, TLS, websocket, DNS, telemetry or updater crates), the UI gains a network-capable
   npm package, or any `tauri-plugin-*` crate appears that is not listed below.
3. The content security policy is missing, has no `default-src`, or allows a remote origin
   (anything other than the app itself, `ipc:` and loopback names).
4. The frontend source calls `fetch`, `XMLHttpRequest`, `WebSocket` or similar, or contains a
   remote URL (scripts, styles, fonts and images are all local).
5. Volt's own Rust source uses a socket or HTTP API (`TcpStream`, `UdpSocket`, WinHTTP, ...), or a
   `windows` crate feature for WinHTTP, WinInet or WebSocket is enabled.
6. The README stops linking to this page.

An entry in the list below for something that is no longer present also fails the check, so the
list stays exact. Type-only crates (`http`, `url`) and the async runtime (`tokio`, `mio`, which
Tauri brings in) are not network clients and are not on the denylist; point 5 is what guards
against Volt code opening a socket.

The allowlist the check reads (everything empty means Volt has no network reach; plugins are
listed because every plugin needs a review, and `tauri-plugin-fs` is only a dependency of the
dialog plugin; `tauri-plugin-notification` shows the local Windows toast after a tray Boost and
makes no request):

<!-- privacy-allowlist:start -->

```json
{
  "networkPermissions": [],
  "networkPackages": [],
  "tauriPlugins": [
    "tauri-plugin-autostart",
    "tauri-plugin-dialog",
    "tauri-plugin-fs",
    "tauri-plugin-notification",
    "tauri-plugin-single-instance"
  ],
  "cspRemoteOrigins": [],
  "frontendRemoteRequests": [],
  "rustNetworkApis": []
}
```

<!-- privacy-allowlist:end -->

How the app is locked down today:

- `apps/desktop/src-tauri/capabilities/default.json` grants the main window only
  `core:event:allow-listen` and `core:event:allow-unlisten`. No plugin permission is granted to the
  page; the plugins run from Rust only.
- The content security policy in `tauri.conf.json` starts from `default-src 'self'` and adds only
  the local IPC channel (`ipc:` and `http://ipc.localhost`). Objects, frames, `<base>` and form
  submission are denied. Style attributes are allowed inline (`style-src 'unsafe-inline'`) because
  React sets them; scripts are not.
- `connect-src` has no remote origin, so even a script that got in could not contact a server
  through the page.
- The development server (`devUrl`) is loopback (`127.0.0.1`) and only used by `pnpm dev`.
  `devCsp` is not set, because Vite's hot reload needs a local websocket; this affects development
  builds only.

## What is outside Volt's control

- **The WebView2 runtime.** Volt draws its window with the Microsoft Edge WebView2 runtime that
  Windows provides. That is Microsoft software with its own updater and settings, and **it makes
  its own network connections that Volt neither makes nor controls** (see the observation below).
  What is known:
  - The installer does not download or install the runtime either: webviewInstallMode is skip,
    so setup makes no network call. Windows 11 ships WebView2. On a Windows 10 machine without it,
    install the runtime from Microsoft first, or Volt will not start.
  - Volt does not install or update the runtime. It passes the runtime three extra browser
    arguments for its own window only (`additionalBrowserArgs` in `tauri.conf.json`):
    `--disable-component-update`, `--disable-domain-reliability` and `--no-pings`, next to the
    defaults of the Tauri runtime (`wry`) that setting the option would otherwise drop:
    `--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection` (among other things this
    turns off the SmartScreen URL reputation lookup) and
    `--autoplay-policy=no-user-gesture-required`. They affect only Volt's own WebView, not Edge
    or other apps, and change nothing on the machine. What they achieved is recorded below.
  - The runtime keeps its user data in `%LOCALAPPDATA%\io.github.tatsuyakari1203.volt\EBWebView`,
    a folder of Volt's own that is not shared with Edge.
  - The page Volt loads cannot reach any server (content security policy), so these connections do
    not come from Volt's page or code.
  - What the runtime sends is not something Volt can promise anything about. Microsoft documents
    that the runtime reports diagnostic data according to the Windows diagnostic data setting and
    updates itself through its own updater (see the Microsoft Learn pages "WebView2 data and
    privacy" under `microsoft-edge/webview2/concepts/privacy` and "Distribution of apps using
    WebView2" under `microsoft-edge/webview2/concepts/distribution`). Volt did not read the
    content of the connections (TLS); the purposes below are inferred from those pages and from
    the destinations, not observed.
- **Windows itself**, antivirus and any network monitoring tool on your machine.
- **Your own actions.** Opening a link from outside Volt, or running a tool Volt only points to,
  is not Volt sending anything.
- **Builds you did not make.** The statements here describe the source in this repository. Only
  build or install from sources you trust.

## Run-time verification

Static checks show what the code and configuration allow; this section records what was observed
in a running app.

<!-- runtime-verification -->

**Method.** `scripts/observe-webview2.ps1` (read-only, repeatable) starts the release build
(`pnpm tauri build --no-bundle`), leaves the window open and alone for 70 seconds, and every second
lists the process tree of `volt-desktop.exe` (the app and its `msedgewebview2.exe` descendants) with
every TCP connection and UDP endpoint those processes own that is not loopback or a listener. It
stops only the processes it started. `-BrowserArgs` puts a test value in
`WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` of that one process; nothing on the machine is changed.

**Result, 2026-11-10 (Windows 11, WebView2 runtime 157.0.4341.42, IPv6 network).** One run of 70 s
per row, repeated where noted. Browser = the `msedgewebview2.exe` browser process; network service =
its `network.mojom.NetworkService` utility process. `volt-desktop.exe` had no TCP connection and no
UDP endpoint in any run, and no process of the tree used UDP or loopback.

| Option tried (own WebView only)                                                                                              | Browser, port 443                      | Network service, port 443    |
| ---------------------------------------------------------------------------------------------------------------------------- | -------------------------------------- | ---------------------------- |
| None (baseline)                                                                                                              | `2603:1046:c01:...::2`, from 3 s, held | `2620:1ec:33::11`, from 61 s |
| `--disable-background-networking`                                                                                            | same                                   | same                         |
| `--disable-sync --disable-breakpad --metrics-recording-only --disable-client-side-phishing-detection --disable-default-apps` | same                                   | same                         |
| `--disable-component-update --disable-domain-reliability --no-pings` (2 runs)                                                | same                                   | gone in both runs            |
| All of the above plus `--disable-features=...,msEdgeUpdate,OptimizationHints,MediaRouter,Translate,msEdgeShopping` (2 runs)  | same                                   | gone in both runs            |
| Shipped: the three-switch option set in `tauri.conf.json` (2 runs)                                                           | same                                   | gone in both runs            |

`2620:1ec:33::11` is one of the two addresses `edge.microsoft.com` resolved to on the test machine
(the Edge services host, plausibly its component or configuration service). The browser process
connection goes to a `2603:1046:...` address, a Microsoft block whose name was not identified; it
begins 3 to 7 s after launch and stays open to the end of every run, whatever switch was set. No
tested switch stopped it, so Volt does not claim it is off. The window and its renderer, GPU and
utility processes started in every shipped run (7 processes); the page itself was not
re-screenshotted for this change.

**What this means.** Volt's own code and page made no connection. The shipped switches stop the
one network-service connection seen in all 3 runs without them; one persistent HTTPS connection
from the WebView2 browser process to Microsoft remains and is outside Volt's control. Each row is
one machine and few runs: a look, not a proof. Repeat the script after a WebView2 major update or
when the loaded page changes.

**Untested options (they change the machine or the runtime, so were not tried).**

- Edge/WebView2 policies in the registry (`HKLM\Software\Policies\Microsoft\Edge` and the
  `EdgeUpdate` policy keys, for example diagnostic data and update policy). Applies to Edge and
  every WebView2 app on the PC, needs administrator rights, may block security updates.
- A firewall rule for `msedgewebview2.exe` or the Microsoft destinations. Machine-wide, can break
  other WebView2 apps, and addresses change.
- A `hosts` file entry for `edge.microsoft.com`. Same trade-offs, plus it is fragile.
- The fixed-version runtime shipped inside Volt. It has no shared updater, so the update check
  would stop, but Volt would then have to ship every WebView2 security update itself and the
  installer grows by over 100 MB. Whether it removes the remaining connection is unknown.
