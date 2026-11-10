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
dialog plugin):

<!-- privacy-allowlist:start -->

```json
{
  "networkPermissions": [],
  "networkPackages": [],
  "tauriPlugins": [
    "tauri-plugin-autostart",
    "tauri-plugin-dialog",
    "tauri-plugin-fs",
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
  - Volt does not configure, install or update the runtime and passes it no browser arguments, so
    it runs with the defaults of the Tauri runtime (`wry`). At the version used here those are
    `--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection` (among other things this turns
    off the SmartScreen URL reputation lookup) and `--autoplay-policy=no-user-gesture-required`.
  - The runtime keeps its user data in `%LOCALAPPDATA%\io.github.tatsuyakari1203.volt\EBWebView`,
    a folder of Volt's own that is not shared with Edge.
  - The page Volt loads cannot reach any server (content security policy), so these connections do
    not come from Volt's page or code.
  - What the runtime sends, to whom and why is not something Volt has verified or can promise
    anything about. Microsoft's documentation and your Windows diagnostic data settings govern it.
    A test with extra Chromium switches (`--disable-background-networking`,
    `--disable-component-update`, `--disable-domain-reliability`, `--no-pings`) did not stop the
    browser process connecting (below), so Volt does not set them and does not claim to.
- **Windows itself**, antivirus and any network monitoring tool on your machine.
- **Your own actions.** Opening a link from outside Volt, or running a tool Volt only points to,
  is not Volt sending anything.
- **Builds you did not make.** The statements here describe the source in this repository. Only
  build or install from sources you trust.

## Run-time verification

Static checks show what the code and configuration allow; this section records what was observed
in a running app.

<!-- runtime-verification -->

**Method.** A release build of the desktop app (`pnpm tauri build --no-bundle`) was started by hand with the window open and left alone for 65 seconds. Every second the
process tree of `volt-desktop.exe` (the app and all of its `msedgewebview2.exe` descendants) was
listed and every TCP connection and UDP endpoint owned by those processes was read with
`Get-NetTCPConnection` and `Get-NetUDPEndpoint`. Anything not on loopback was recorded. Only the
processes the test started were stopped afterwards. The window was also screenshotted to confirm
the interface renders under the content security policy.

**Result, 2026-11-10 (Windows 11, WebView2 runtime 157.0.4341.42, IPv6 network).** Four runs; the
three on the shipped configuration agree.

| What                                            | Observed                                                                                                                                                          |
| ----------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `volt-desktop.exe` (Volt's own process)         | No TCP connection and no UDP endpoint of any kind, in every run                                                                                                   |
| Loopback traffic                                | None seen from any process of the tree                                                                                                                            |
| UDP endpoints, whole tree                       | None                                                                                                                                                              |
| WebView2 browser process (`msedgewebview2.exe`) | Two HTTPS connections (port 443) to Microsoft addresses about 7 to 14 s after launch, closed within 30 s                                                          |
| WebView2 network service process                | One HTTPS connection to `2620:1ec:33::11`, about 64 s after launch (in the third run the same address was seen from a WebView2 child whose type was not recorded) |

The destinations could not be named by reverse lookup. `2620:1ec:33::11` is one of the two
addresses `edge.microsoft.com` resolved to on the test machine; the `2603:1046:c01:...` addresses
of the browser process were not identified. Nothing about the content of the connections was
inspected (they are TLS). So the honest statement is: **Volt's own code and page made no
connection; the WebView2 runtime that Windows provides did connect to Microsoft services.** This is
the runtime's behaviour, it is outside the check above, and it is why the first line of this page
is limited to what Volt does. Whether a machine with the runtime's data collection or updates
turned off by policy behaves differently was not tested.

This is one machine, one minute and one WebView2 version, repeated three times. It is a look, not
a proof. Repeat it after a WebView2 major update or when the loaded page changes.
