#!/usr/bin/env node
// Checks the "Volt works offline and sends nothing" guarantee against docs/privacy.md.
//
// docs/privacy.md carries a machine-readable allowlist (between the privacy-allowlist markers) of
// everything that may give Volt network reach. This script compares reality with that list and
// fails on any difference in either direction:
//
//   - a network permission or network config in a Tauri capability or tauri.conf.json,
//   - a network-capable crate (HTTP, TLS, websocket, telemetry, updater...) in the shipped
//     dependency graph, or a network-capable npm package in the UI's dependencies,
//   - an unlisted tauri-plugin-* crate (every plugin needs a privacy review, network or not),
//   - a remote origin (or a missing policy) in the content security policy,
//   - a remote request or remote URL in the frontend source,
//   - a raw socket or HTTP API in Volt's own Rust source, or a WinHTTP/WinInet/WebSocket feature,
//   - the README no longer pointing at the privacy statement.
//
// Type-only crates such as `http`, `url`, `mio` or `tokio` are deliberately not on the denylist:
// they cannot connect on their own, and Volt's own sources are scanned for the socket APIs.
// The list entry for something that is gone is also a failure, so the statement stays exact.

import { execFileSync } from 'node:child_process';
import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import { dirname, join, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const PRIVACY_DOC = 'docs/privacy.md';
const WINDOWS_TARGET = 'x86_64-pc-windows-msvc';

/** Crates that can open a connection or report data. Matched by exact name. */
export const NETWORK_CRATES = new Set([
  'reqwest',
  'reqwest-middleware',
  'hyper',
  'hyper-util',
  'hyper-rustls',
  'hyper-tls',
  'h2',
  'h3',
  'ureq',
  'attohttpc',
  'isahc',
  'curl',
  'curl-sys',
  'surf',
  'minreq',
  'tiny_http',
  'tonic',
  'rustls',
  'native-tls',
  'openssl',
  'openssl-sys',
  'schannel',
  'webpki-roots',
  'rustls-native-certs',
  'tungstenite',
  'tokio-tungstenite',
  'async-tungstenite',
  'quinn',
  'quinn-proto',
  'trust-dns-resolver',
  'hickory-resolver',
  'hickory-client',
  'mdns-sd',
  'libp2p',
  'winhttp',
  'wininet',
  'sentry',
  'sentry-core',
  'opentelemetry',
  'opentelemetry-otlp',
  'posthog-rs',
  'tauri-plugin-http',
  'tauri-plugin-websocket',
  'tauri-plugin-updater',
  'tauri-plugin-upload',
  'tauri-plugin-opener',
  'tauri-plugin-shell',
  'tauri-plugin-oauth',
  'tauri-plugin-localhost',
  'tauri-plugin-sentry',
  'tauri-plugin-aptabase',
]);

/** UI packages that can reach the network. Matched by exact name. */
export const NETWORK_NPM = new Set([
  '@tauri-apps/plugin-http',
  '@tauri-apps/plugin-websocket',
  '@tauri-apps/plugin-updater',
  '@tauri-apps/plugin-upload',
  '@tauri-apps/plugin-opener',
  '@tauri-apps/plugin-shell',
  'axios',
  'node-fetch',
  'cross-fetch',
  'isomorphic-fetch',
  'ky',
  'got',
  'undici',
  'superagent',
  'ws',
  'socket.io-client',
  '@sentry/browser',
  '@sentry/react',
  'posthog-js',
  'mixpanel-browser',
  '@vercel/analytics',
]);

/** Tauri plugins whose permissions reach the network or open remote content. */
const NETWORK_PLUGINS = new Set(['http', 'websocket', 'updater', 'upload', 'opener', 'shell']);

/** Core permissions that let the UI create a window or webview, which can load any URL. */
const NETWORK_CORE_PERMISSIONS = new Set([
  'core:webview:allow-create-webview',
  'core:webview:allow-create-webview-window',
  'core:window:allow-create',
]);

/** Permission names that mean a request, whatever plugin they belong to. */
const NETWORK_PERMISSION_WORDS = /(?:^|-)(?:fetch|connect|download|upload)(?:-|$)/;

/** `windows` crate features that are HTTP-level client APIs. Raw WinSock is covered by the source scan. */
const NETWORK_WINDOWS_FEATURES = new Set([
  'Win32_Networking_WinHttp',
  'Win32_Networking_WinInet',
  'Win32_Networking_WebSocket',
]);

/** Frontend APIs that make a request. */
const FRONTEND_APIS = [
  ['fetch', /\bfetch\s*\(/],
  ['XMLHttpRequest', /\bXMLHttpRequest\b/],
  ['WebSocket', /\bWebSocket\b/],
  ['EventSource', /\bEventSource\b/],
  ['WebTransport', /\bWebTransport\b/],
  ['RTCPeerConnection', /\bRTCPeerConnection\b/],
  ['sendBeacon', /\bsendBeacon\b/],
  ['importScripts', /\bimportScripts\b/],
];

/** Rust APIs that open a socket or call an HTTP stack directly. */
const RUST_APIS = [
  ['TcpStream', /\bTcpStream\b/],
  ['TcpListener', /\bTcpListener\b/],
  ['UdpSocket', /\bUdpSocket\b/],
  ['ToSocketAddrs', /\bToSocketAddrs\b/],
  ['tokio::net', /\btokio::net\b/],
  ['WinHttp', /\bWinHttp[A-Z]\w*/],
  ['InternetOpen', /\bInternetOpen\w*/],
  ['URLDownloadToFile', /\bURLDownloadToFile\w*/],
  ['WSAStartup', /\bWSAStartup\b/],
];

const LOOPBACK_HOSTS = new Set(['localhost', '127.0.0.1', '[::1]', '::1']);
const CSP_LOCAL_SCHEMES = new Set(['ipc:', 'asset:', 'data:', 'blob:', 'tauri:']);
const XML_NAMESPACE = /^https?:\/\/www\.w3\.org\/(?:2000\/svg|1999\/xlink|1999\/xhtml)\b/;

const HOW_TO_FIX = `Update ${PRIVACY_DOC} (the statement and its allowlist) in the same change.`;

/** True for localhost, a `.localhost` name and loopback addresses. */
const isLoopbackHost = (host) => {
  const h = host.toLowerCase().replace(/:\d+$/, '');
  return LOOPBACK_HOSTS.has(h) || h.endsWith('.localhost');
};

/**
 * Reads the allowlist out of the privacy statement.
 * @param {string} markdown
 * @returns {Record<string, string[]>}
 */
export function parseAllowlist(markdown) {
  const match =
    /<!--\s*privacy-allowlist:start\s*-->\s*```json\s*([\s\S]*?)```\s*<!--\s*privacy-allowlist:end\s*-->/.exec(
      markdown,
    );
  if (!match) {
    throw new Error(
      `${PRIVACY_DOC} has no allowlist block between privacy-allowlist:start and privacy-allowlist:end markers.`,
    );
  }
  const value = JSON.parse(match[1]);
  const keys = [
    'networkPermissions',
    'networkPackages',
    'tauriPlugins',
    'cspRemoteOrigins',
    'frontendRemoteRequests',
    'rustNetworkApis',
  ];
  for (const key of keys) {
    if (!Array.isArray(value[key]) || value[key].some((entry) => typeof entry !== 'string')) {
      throw new Error(`${PRIVACY_DOC} allowlist: "${key}" must be an array of strings.`);
    }
  }
  return value;
}

/**
 * Compares what was found with what the allowlist says, in both directions.
 * @param {string} label what the list covers, for messages
 * @param {Iterable<string>} found
 * @param {string[]} listed
 * @param {string} describeFound how to describe an unlisted item
 */
function compare(label, found, listed, describeFound) {
  const foundSet = new Set(found);
  const listedSet = new Set(listed);
  const out = [];
  for (const item of [...foundSet].sort()) {
    if (!listedSet.has(item)) out.push(`${describeFound(item)} ${HOW_TO_FIX}`);
  }
  for (const item of [...listedSet].sort()) {
    if (!foundSet.has(item)) {
      out.push(
        `${PRIVACY_DOC} lists ${label} "${item}" but it is not present. Remove it from the allowlist.`,
      );
    }
  }
  return out;
}

/**
 * Splits a content security policy into directives.
 * @param {string} csp
 * @returns {Array<{ name: string, sources: string[] }>}
 */
export function parseCsp(csp) {
  return csp
    .split(';')
    .map((part) => part.trim().split(/\s+/).filter(Boolean))
    .filter((tokens) => tokens.length > 0)
    .map(([name, ...sources]) => ({ name: name.toLowerCase(), sources }));
}

/**
 * Returns the sources of a policy that could load something from outside the machine.
 * @param {string} csp
 * @returns {string[]}
 */
export function remoteCspSources(csp) {
  const remote = [];
  for (const { sources } of parseCsp(csp)) {
    for (const source of sources) {
      if (source.startsWith("'")) continue; // keywords, nonces and hashes
      const lower = source.toLowerCase();
      if (CSP_LOCAL_SCHEMES.has(lower)) continue;
      if (/^[a-z][a-z0-9+.-]*:$/.test(lower)) {
        remote.push(source); // a bare scheme such as https: or wss:
        continue;
      }
      const withHost = /^([a-z][a-z0-9+.-]*):\/\/([^/]*)/.exec(lower);
      if (withHost) {
        if (['ipc', 'asset', 'tauri'].includes(withHost[1]) || isLoopbackHost(withHost[2]))
          continue;
        remote.push(source);
        continue;
      }
      remote.push(source); // `*`, a bare host, or anything unrecognised: treat as remote
    }
  }
  return [...new Set(remote)];
}

/**
 * Checks one policy string and returns the remote sources plus structural problems.
 * @param {unknown} csp
 * @param {string} label
 * @returns {{ remote: string[], problems: string[] }}
 */
function inspectCsp(csp, label) {
  if (typeof csp === 'object' && csp !== null) {
    // Tauri also accepts a map of directive to sources.
    csp = Object.entries(csp)
      .map(([name, sources]) => `${name} ${Array.isArray(sources) ? sources.join(' ') : sources}`)
      .join('; ');
  }
  if (typeof csp !== 'string' || csp.trim() === '') {
    return {
      remote: [],
      problems: [
        `${label} is missing or empty: without a content security policy the page may load and contact anything. Set one that starts from default-src 'self'.`,
      ],
    };
  }
  const problems = [];
  const directives = parseCsp(csp);
  if (!directives.some((d) => d.name === 'default-src')) {
    problems.push(
      `${label} has no default-src directive, so unlisted resource types are unrestricted.`,
    );
  }
  return { remote: remoteCspSources(csp), problems };
}

/**
 * Checks the Tauri capability files.
 * @param {Array<{ path: string, json: any }>} capabilities
 * @returns {Map<string, string>} permission id mapped to a description
 */
export function networkPermissionsIn(capabilities) {
  const found = new Map();
  for (const { path, json } of capabilities) {
    if (json && typeof json === 'object' && json.remote) {
      found.set(
        `remote:${path}`,
        `${path} lets remote pages call Tauri commands ("remote" is set).`,
      );
    }
    for (const entry of json?.permissions ?? []) {
      const id = typeof entry === 'string' ? entry : entry?.identifier;
      if (typeof id !== 'string') continue;
      const [plugin, ...rest] = id.split(':');
      const name = rest.join(':');
      if (
        NETWORK_PLUGINS.has(plugin) ||
        NETWORK_CORE_PERMISSIONS.has(id) ||
        NETWORK_PERMISSION_WORDS.test(name.split(':').pop() ?? '')
      ) {
        found.set(id, `${path} grants the network-capable permission "${id}".`);
      }
    }
  }
  return found;
}

/**
 * Checks tauri.conf.json outside the policy: remote windows, remote IPC, updater, network plugins.
 * @param {any} conf
 * @returns {Map<string, string>}
 */
export function networkConfigIn(conf) {
  const found = new Map();
  const remoteUrl = (value) => {
    if (typeof value !== 'string') return false;
    const m = /^(?:https?|wss?|ftp):\/\/([^/]+)/i.exec(value);
    return m !== null && !isLoopbackHost(m[1]);
  };
  if (remoteUrl(conf?.build?.devUrl)) {
    found.set(
      'build.devUrl',
      `tauri.conf.json build.devUrl points at a remote host: ${conf.build.devUrl}`,
    );
  }
  if (remoteUrl(conf?.build?.frontendDist)) {
    found.set('build.frontendDist', 'tauri.conf.json build.frontendDist points at a remote host.');
  }
  for (const w of conf?.app?.windows ?? []) {
    if (remoteUrl(w?.url)) {
      found.set(
        `window.url:${w.label ?? '?'}`,
        `tauri.conf.json window "${w.label}" loads a remote URL.`,
      );
    }
  }
  const security = conf?.app?.security ?? {};
  const remoteIpc = security.dangerousRemoteDomainIpcAccess;
  if (Array.isArray(remoteIpc) && remoteIpc.length > 0) {
    found.set(
      'security.dangerousRemoteDomainIpcAccess',
      'tauri.conf.json lets remote domains call Tauri commands.',
    );
  }
  if (security.dangerousDisableAssetCspModification) {
    found.set(
      'security.dangerousDisableAssetCspModification',
      'tauri.conf.json stops Tauri from applying the content security policy to its own assets.',
    );
  }
  for (const name of Object.keys(conf?.plugins ?? {})) {
    if (NETWORK_PLUGINS.has(name)) {
      found.set(
        `plugins.${name}`,
        `tauri.conf.json configures the network-capable plugin "${name}".`,
      );
    }
  }
  if (conf?.bundle?.createUpdaterArtifacts) {
    found.set(
      'bundle.createUpdaterArtifacts',
      'tauri.conf.json builds updater artifacts (an auto-updater).',
    );
  }
  return found;
}

/** Removes comments that cannot hide a request. Conservative: trailing comments are kept. */
function stripComments(text, path) {
  if (path.endsWith('.html')) return text.replace(/<!--[\s\S]*?-->/g, '');
  const noBlock = text.replace(/(^|\s)\/\*[\s\S]*?\*\//g, '$1');
  if (path.endsWith('.css')) return noBlock;
  return noBlock.replace(/^\s*\/\/.*$/gm, '');
}

/**
 * Scans frontend sources for request APIs, remote URLs and network packages.
 * @param {Array<{ path: string, text: string }>} files
 * @returns {Map<string, string>}
 */
export function frontendRequestsIn(files) {
  const found = new Map();
  const urlPattern = /\b(?:https?|wss?|ftp):\/\/[^\s'"`)<>\\]+/gi;
  const protocolRelative =
    /(?:src|href|action|url\(|@import)\s*=?\s*["']?\s*\/\/[a-z0-9-]+\.[a-z0-9.-]+/gi;
  const pluginImport =
    /from\s+['"]([^'"]+)['"]|import\(\s*['"]([^'"]+)['"]\s*\)|require\(\s*['"]([^'"]+)['"]\s*\)/g;
  for (const { path, text } of files) {
    const code = stripComments(text, path);
    for (const [id, pattern] of FRONTEND_APIS) {
      if (pattern.test(code)) found.set(id, `${path} uses ${id}, which can make a request.`);
    }
    for (const url of code.match(urlPattern) ?? []) {
      if (XML_NAMESPACE.test(url)) continue;
      const m = /^[a-z]+:\/\/([^/\s]+)/i.exec(url);
      if (m && isLoopbackHost(m[1])) continue;
      const origin = /^[a-z]+:\/\/[^/\s]+/i.exec(url)?.[0] ?? url;
      found.set(origin, `${path} refers to the remote address ${url}.`);
    }
    for (const rel of code.match(protocolRelative) ?? []) {
      found.set(rel.trim(), `${path} refers to a protocol-relative remote address: ${rel.trim()}`);
    }
    for (const m of code.matchAll(pluginImport)) {
      const spec = m[1] ?? m[2] ?? m[3];
      if (NETWORK_NPM.has(spec))
        found.set(`npm:${spec}`, `${path} imports the network package ${spec}.`);
    }
  }
  return found;
}

/**
 * Scans Volt's own Rust sources for raw socket and HTTP APIs.
 * @param {Array<{ path: string, text: string }>} files
 * @returns {Map<string, string>}
 */
export function rustNetworkApisIn(files) {
  const found = new Map();
  for (const { path, text } of files) {
    const code = text.replace(/(^|\s)\/\*[\s\S]*?\*\//g, '$1').replace(/^\s*\/\/.*$/gm, '');
    for (const [id, pattern] of RUST_APIS) {
      if (pattern.test(code)) found.set(id, `${path} uses ${id}, which opens a connection.`);
    }
  }
  return found;
}

/**
 * Runs every check.
 * @param {{
 *   crates: string[],
 *   crateFeatures: Record<string, string[]>,
 *   npmDeps: string[],
 *   capabilities: Array<{ path: string, json: any }>,
 *   tauriConf: any,
 *   frontend: Array<{ path: string, text: string }>,
 *   rust: Array<{ path: string, text: string }>,
 *   readme: string,
 * }} reality
 * @param {Record<string, string[]>} allow
 * @returns {string[]} violations, empty when the guarantee holds
 */
export function runChecks(reality, allow) {
  const out = [];

  // Capabilities and tauri.conf.json, outside the policy.
  const permissions = networkPermissionsIn(reality.capabilities);
  for (const [id, text] of networkConfigIn(reality.tauriConf)) permissions.set(id, text);
  out.push(
    ...compare('network permission', permissions.keys(), allow.networkPermissions, (id) =>
      permissions.get(id),
    ),
  );

  // Dependency graph.
  const packages = new Map();
  for (const name of reality.crates) {
    if (NETWORK_CRATES.has(name)) {
      packages.set(
        name,
        `The shipped dependency graph contains the network-capable crate "${name}".`,
      );
    }
  }
  for (const [name, features] of Object.entries(reality.crateFeatures)) {
    if (!name.startsWith('windows')) continue;
    for (const feature of features) {
      if (NETWORK_WINDOWS_FEATURES.has(feature)) {
        const id = `${name}:${feature}`;
        packages.set(id, `The crate "${name}" enables the HTTP-level feature "${feature}".`);
      }
    }
  }
  for (const name of reality.npmDeps) {
    if (NETWORK_NPM.has(name)) {
      packages.set(`npm:${name}`, `The UI depends on the network-capable package "${name}".`);
    }
  }
  out.push(
    ...compare('network package', packages.keys(), allow.networkPackages, (id) => packages.get(id)),
  );

  // Every plugin needs a review, network or not.
  const plugins = reality.crates.filter(
    (n) => n.startsWith('tauri-plugin-') && !NETWORK_CRATES.has(n),
  );
  out.push(
    ...compare(
      'plugin',
      plugins,
      allow.tauriPlugins,
      (id) => `The plugin crate "${id}" is in the shipped dependency graph but not reviewed.`,
    ),
  );

  // Content security policy.
  const policies = [['app.security.csp', reality.tauriConf?.app?.security?.csp]];
  if (reality.tauriConf?.app?.security?.devCsp != null) {
    policies.push(['app.security.devCsp', reality.tauriConf.app.security.devCsp]);
  }
  const cspRemote = new Set();
  for (const [label, csp] of policies) {
    const { remote, problems } = inspectCsp(csp, `tauri.conf.json ${label}`);
    for (const problem of problems) out.push(`${problem} ${HOW_TO_FIX}`);
    for (const source of remote) cspRemote.add(source);
  }
  out.push(
    ...compare(
      'CSP origin',
      cspRemote,
      allow.cspRemoteOrigins,
      (id) => `The content security policy allows the remote source "${id}".`,
    ),
  );

  // Frontend source and Rust source.
  const requests = frontendRequestsIn(reality.frontend);
  out.push(
    ...compare('frontend request', requests.keys(), allow.frontendRemoteRequests, (id) =>
      requests.get(id),
    ),
  );
  const apis = rustNetworkApisIn(reality.rust);
  out.push(
    ...compare('Rust network API', apis.keys(), allow.rustNetworkApis, (id) => apis.get(id)),
  );

  // The README must lead to the statement.
  if (!reality.readme.includes(PRIVACY_DOC)) {
    out.push(
      `README.md does not link to ${PRIVACY_DOC}. State what Volt sends and link the privacy statement.`,
    );
  }
  return out;
}

// --- Loading the real repository ---------------------------------------------------------------

const walk = (dir, accept, skip = () => false) => {
  const files = [];
  if (!existsSync(dir)) return files;
  for (const name of readdirSync(dir)) {
    const full = join(dir, name);
    if (skip(full, name)) continue;
    if (statSync(full).isDirectory()) files.push(...walk(full, accept, skip));
    else if (accept(name)) files.push(full);
  }
  return files;
};

const readFiles = (root, paths) =>
  paths.map((p) => ({
    path: relative(root, p).split(sep).join('/'),
    text: readFileSync(p, 'utf8'),
  }));

/** Crate names and enabled features reachable from the workspace through normal and build edges. */
function shippedCrates(root) {
  const metadata = JSON.parse(
    execFileSync(
      'cargo',
      ['metadata', '--format-version', '1', '--locked', '--filter-platform', WINDOWS_TARGET],
      {
        cwd: root,
        encoding: 'utf8',
        maxBuffer: 256 * 1024 * 1024,
      },
    ),
  );
  const nameOf = new Map(metadata.packages.map((p) => [p.id, p.name]));
  const nodes = new Map(metadata.resolve.nodes.map((n) => [n.id, n]));
  const seen = new Set();
  const stack = [...metadata.workspace_members];
  while (stack.length > 0) {
    const id = stack.pop();
    if (seen.has(id)) continue;
    seen.add(id);
    for (const dep of nodes.get(id)?.deps ?? []) {
      if (dep.dep_kinds.some((k) => k.kind !== 'dev')) stack.push(dep.pkg);
    }
  }
  const crates = new Set();
  const crateFeatures = {};
  for (const id of seen) {
    const name = nameOf.get(id);
    crates.add(name);
    crateFeatures[name] = [
      ...new Set([...(crateFeatures[name] ?? []), ...(nodes.get(id)?.features ?? [])]),
    ];
  }
  return { crates: [...crates], crateFeatures };
}

/** Collects the facts the checks compare against the allowlist. */
export function loadReality(root) {
  const desktop = join(root, 'apps', 'desktop');
  const tauri = join(desktop, 'src-tauri');
  const sourceSkip = (_full, name) =>
    name === 'node_modules' ||
    name === 'target' ||
    name === 'dist' ||
    /\.test\.[jt]sx?$/.test(name) ||
    name === 'test';
  const frontendPaths = [
    join(desktop, 'index.html'),
    ...walk(join(desktop, 'src'), (n) => /\.(?:[jt]sx?|css|html)$/.test(n), sourceSkip),
  ].filter(existsSync);
  const rustSkip = (_full, name) => name === 'target' || name === 'node_modules';
  const rustPaths = [
    ...walk(join(root, 'crates'), (n) => n.endsWith('.rs'), rustSkip),
    ...walk(join(tauri, 'src'), (n) => n.endsWith('.rs'), rustSkip),
    ...walk(join(tauri, 'tests'), (n) => n.endsWith('.rs'), rustSkip),
  ];
  const capabilityPaths = walk(
    join(tauri, 'capabilities'),
    (n) => n.endsWith('.json') || n.endsWith('.toml'),
  );
  const uiPackage = JSON.parse(readFileSync(join(desktop, 'package.json'), 'utf8'));
  return {
    ...shippedCrates(root),
    npmDeps: Object.keys(uiPackage.dependencies ?? {}),
    capabilities: capabilityPaths.map((p) => ({
      path: relative(root, p).split(sep).join('/'),
      json: p.endsWith('.json')
        ? JSON.parse(readFileSync(p, 'utf8'))
        : tomlPermissions(readFileSync(p, 'utf8')),
    })),
    tauriConf: JSON.parse(readFileSync(join(tauri, 'tauri.conf.json'), 'utf8')),
    frontend: readFiles(root, frontendPaths),
    rust: readFiles(root, rustPaths),
    readme: readFileSync(join(root, 'README.md'), 'utf8'),
  };
}

/** Minimal reader for TOML capability files: the permission list and a `remote` table. */
function tomlPermissions(text) {
  const permissions = [...text.matchAll(/"([a-z0-9-]+(?::[a-z0-9:-]+)?)"/gi)].map((m) => m[1]);
  return { permissions, remote: /^\s*\[remote\]/m.test(text) ? {} : undefined };
}

function main() {
  const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
  let allow;
  try {
    allow = parseAllowlist(readFileSync(join(root, PRIVACY_DOC), 'utf8'));
  } catch (error) {
    console.error(`Privacy check failed: ${error.message}`);
    process.exit(1);
  }
  const reality = loadReality(root);
  const violations = runChecks(reality, allow);
  if (violations.length > 0) {
    console.error(`Privacy check failed (${violations.length}):`);
    for (const v of violations) console.error(`  - ${v}`);
    process.exit(1);
  }
  console.log(
    `Privacy check passed: ${reality.crates.length} crates, ${reality.capabilities.length} capability file(s), ` +
      `${reality.frontend.length} frontend and ${reality.rust.length} Rust source files, no network reach beyond ${PRIVACY_DOC}.`,
  );
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main();
