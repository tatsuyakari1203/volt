// Tests for scripts/check-privacy.mjs. Run with `node --test scripts/`.
// Each kind of violation gets a case that must fail, plus cases that must stay quiet.

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { describe, it } from 'node:test';
import { fileURLToPath } from 'node:url';

import { parseAllowlist, remoteCspSources, runChecks } from './check-privacy.mjs';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');

const CLEAN_CSP =
  "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; connect-src 'self' ipc: http://ipc.localhost";

const emptyAllow = () => ({
  networkPermissions: [],
  networkPackages: [],
  tauriPlugins: ['tauri-plugin-dialog'],
  cspRemoteOrigins: [],
  frontendRemoteRequests: [],
  rustNetworkApis: [],
});

/** A repository that satisfies the guarantee. Tests change one thing at a time. */
const cleanReality = () => ({
  crates: ['volt-core', 'tauri', 'tauri-plugin-dialog', 'http', 'url', 'tokio', 'mio', 'socket2'],
  crateFeatures: { windows: ['Win32_Foundation'], 'windows-sys': ['Win32_Networking_WinSock'] },
  npmDeps: ['react', 'react-dom', '@tauri-apps/api'],
  capabilities: [
    {
      path: 'capabilities/default.json',
      json: { permissions: ['core:event:allow-listen', 'core:event:allow-unlisten'] },
    },
  ],
  tauriConf: {
    build: { devUrl: 'http://127.0.0.1:1420', frontendDist: '../dist' },
    app: { windows: [{ label: 'main', create: false }], security: { csp: CLEAN_CSP } },
  },
  frontend: [
    {
      path: 'src/App.tsx',
      text: '// see https://example.com/docs\nconst a = <svg xmlns="http://www.w3.org/2000/svg" />;\nconst b = "http://localhost:1420";\n',
    },
  ],
  rust: [
    {
      path: 'crates/volt-win/src/lib.rs',
      text: '// A TcpStream would be wrong here.\nfn main() {}\n',
    },
  ],
  readme: 'See [privacy](docs/privacy.md).',
});

const check = (change, allow = emptyAllow()) => {
  const reality = cleanReality();
  change?.(reality);
  return runChecks(reality, allow);
};

describe('a clean repository', () => {
  it('passes, and type-only crates do not fire', () => {
    assert.deepEqual(check(), []);
  });
});

describe('network permissions', () => {
  it('fails on a network plugin permission', () => {
    const v = check((r) => r.capabilities[0].json.permissions.push('http:default'));
    assert.equal(v.length, 1);
    assert.match(v[0], /http:default/);
    assert.match(v[0], /docs\/privacy\.md/);
  });

  it('fails on opener, shell and updater permissions given as objects', () => {
    const v = check((r) =>
      r.capabilities[0].json.permissions.push(
        { identifier: 'opener:allow-open-url' },
        'shell:allow-open',
        'updater:default',
      ),
    );
    assert.equal(v.length, 3);
  });

  it('fails on creating a window or webview', () => {
    const v = check((r) =>
      r.capabilities[0].json.permissions.push('core:webview:allow-create-webview-window'),
    );
    assert.equal(v.length, 1);
  });

  it('fails on a connect or download permission of any plugin', () => {
    assert.equal(
      check((r) => r.capabilities[0].json.permissions.push('fs:allow-download')).length,
      1,
    );
    assert.equal(
      check((r) => r.capabilities[0].json.permissions.push('x:allow-connect')).length,
      1,
    );
  });

  it('does not fire on local permissions', () => {
    const v = check((r) =>
      r.capabilities[0].json.permissions.push(
        'dialog:allow-save',
        'core:window:allow-set-title',
        'fs:allow-open',
      ),
    );
    assert.deepEqual(v, []);
  });

  it('fails when remote pages may call commands', () => {
    const v = check((r) => {
      r.capabilities[0].json.remote = { urls: ['https://example.com'] };
    });
    assert.equal(v.length, 1);
  });

  it('passes once the privacy statement lists it, and fails when the listing goes stale', () => {
    const allow = { ...emptyAllow(), networkPermissions: ['http:default'] };
    assert.deepEqual(
      check((r) => r.capabilities[0].json.permissions.push('http:default'), allow),
      [],
    );
    const stale = check(null, allow);
    assert.equal(stale.length, 1);
    assert.match(stale[0], /not present/);
  });

  it('fails on tauri.conf.json network config', () => {
    assert.equal(
      check((r) => (r.tauriConf.plugins = { updater: { endpoints: ['https://x.test'] } })).length,
      1,
    );
    assert.equal(check((r) => (r.tauriConf.build.devUrl = 'https://dev.example.com')).length, 1);
    assert.equal(check((r) => (r.tauriConf.app.windows[0].url = 'https://example.com')).length, 1);
    assert.equal(
      check(
        (r) => (r.tauriConf.app.security.dangerousRemoteDomainIpcAccess = [{ domain: 'x.com' }]),
      ).length,
      1,
    );
    assert.equal(check((r) => (r.tauriConf.bundle = { createUpdaterArtifacts: true })).length, 1);
  });
});

describe('dependency graph', () => {
  it('fails on a network-capable crate', () => {
    for (const crate of ['reqwest', 'hyper', 'ureq', 'rustls', 'tauri-plugin-http', 'sentry']) {
      const v = check((r) => r.crates.push(crate));
      assert.equal(v.length, 1, crate);
      assert.match(v[0], new RegExp(crate));
    }
  });

  it('fails on a WinHTTP or WinInet feature of the windows crate', () => {
    const v = check((r) => r.crateFeatures.windows.push('Win32_Networking_WinHttp'));
    assert.equal(v.length, 1);
    assert.match(v[0], /WinHttp/);
  });

  it('fails on a network npm package', () => {
    for (const pkg of ['axios', '@tauri-apps/plugin-http', 'posthog-js']) {
      assert.equal(check((r) => r.npmDeps.push(pkg)).length, 1, pkg);
    }
  });

  it('fails on a plugin that is not in the privacy list, even a local one', () => {
    const v = check((r) => r.crates.push('tauri-plugin-store'));
    assert.equal(v.length, 1);
    assert.match(v[0], /tauri-plugin-store/);
  });

  it('is satisfied by the listing, and the listing may not go stale', () => {
    const allow = { ...emptyAllow(), networkPackages: ['reqwest'] };
    assert.deepEqual(
      check((r) => r.crates.push('reqwest'), allow),
      [],
    );
    assert.equal(check(null, allow).length, 1);
    const stalePlugin = {
      ...emptyAllow(),
      tauriPlugins: ['tauri-plugin-dialog', 'tauri-plugin-gone'],
    };
    assert.equal(check(null, stalePlugin).length, 1);
  });
});

describe('content security policy', () => {
  it('fails when the policy is missing', () => {
    assert.equal(check((r) => delete r.tauriConf.app.security.csp).length, 1);
    assert.equal(check((r) => (r.tauriConf.app.security.csp = null)).length, 1);
    assert.equal(check((r) => (r.tauriConf.app.security.csp = '')).length, 1);
  });

  it('fails when there is no default-src', () => {
    assert.equal(check((r) => (r.tauriConf.app.security.csp = "script-src 'self'")).length, 1);
  });

  it('fails on a remote origin', () => {
    const v = check((r) => (r.tauriConf.app.security.csp = `${CLEAN_CSP} https://cdn.example.com`));
    assert.equal(v.length, 1);
    assert.match(v[0], /cdn\.example\.com/);
  });

  it('fails on wildcards, bare schemes and bare hosts', () => {
    for (const source of ['*', 'https:', 'wss:', 'example.com', 'wss://api.example.com']) {
      const v = check(
        (r) => (r.tauriConf.app.security.csp = `${CLEAN_CSP}; img-src 'self' ${source}`),
      );
      assert.equal(v.length, 1, source);
    }
  });

  it('also checks devCsp', () => {
    const v = check(
      (r) => (r.tauriConf.app.security.devCsp = "default-src 'self' https://x.example"),
    );
    assert.equal(v.length, 1);
  });

  it('allows self, ipc, data, loopback and nonces', () => {
    const csp =
      "default-src 'self'; img-src 'self' data: blob:; connect-src ipc: http://ipc.localhost http://127.0.0.1:1420 ws://localhost:1420 'nonce-abc'";
    assert.deepEqual(remoteCspSources(csp), []);
  });

  it('is satisfied by the listing', () => {
    const allow = { ...emptyAllow(), cspRemoteOrigins: ['https://cdn.example.com'] };
    assert.deepEqual(
      check((r) => (r.tauriConf.app.security.csp = `${CLEAN_CSP} https://cdn.example.com`), allow),
      [],
    );
  });
});

describe('frontend source', () => {
  const withSource =
    (text, path = 'src/features/x.tsx') =>
    (r) =>
      r.frontend.push({ path, text });

  it('fails on fetch, XMLHttpRequest, WebSocket, EventSource and sendBeacon', () => {
    for (const text of [
      'await fetch("/api")',
      'new XMLHttpRequest()',
      'new WebSocket(url)',
      'new EventSource(url)',
      'navigator.sendBeacon(u, d)',
    ]) {
      assert.equal(check(withSource(text)).length, 1, text);
    }
  });

  it('fails on a remote URL in code, markup and styles', () => {
    assert.equal(check(withSource('const u = "https://example.com/a.js";')).length, 1);
    assert.equal(
      check(withSource('<script src="https://cdn.example.com/x.js"></script>', 'index.html'))
        .length,
      1,
    );
    assert.equal(
      check(withSource('@import url("https://fonts.googleapis.com/css");', 'src/index.css')).length,
      1,
    );
    assert.equal(check(withSource('<img src="//cdn.example.com/a.png">', 'index.html')).length, 1);
  });

  it('fails on importing a network package', () => {
    assert.equal(
      check(withSource("import { fetch } from '@tauri-apps/plugin-http';")).length >= 1,
      true,
    );
  });

  it('does not fire on comments, XML namespaces, loopback or words that only look similar', () => {
    const quiet = [
      '// fetch("https://example.com")',
      '/* https://example.com */',
      '<svg xmlns="http://www.w3.org/2000/svg"></svg>',
      'const dev = "http://127.0.0.1:1420";',
      'refetch(); prefetch(); const fetched = 1;',
    ];
    for (const text of quiet) assert.deepEqual(check(withSource(text)), [], text);
  });

  it('is satisfied by the listing', () => {
    const allow = { ...emptyAllow(), frontendRemoteRequests: ['https://example.com'] };
    assert.deepEqual(check(withSource('const u = "https://example.com/a";'), allow), []);
  });
});

describe('Rust source', () => {
  const withRust = (text) => (r) => r.rust.push({ path: 'crates/volt-win/src/x.rs', text });

  it('fails on sockets and HTTP APIs', () => {
    for (const text of [
      'let s = std::net::TcpStream::connect(a);',
      'let s = UdpSocket::bind(a);',
      'let l = TcpListener::bind(a);',
      'use tokio::net::TcpStream;',
      'unsafe { WinHttpOpen(); }',
      'unsafe { InternetOpenW(); }',
    ]) {
      assert.ok(check(withRust(text)).length >= 1, text);
    }
  });

  it('does not fire on comments or on type-only std::net use', () => {
    assert.deepEqual(check(withRust('/// Never opens a TcpStream.\nuse std::net::IpAddr;\n')), []);
  });
});

describe('README', () => {
  it('fails when it stops linking to the privacy statement', () => {
    const v = check((r) => (r.readme = 'Nothing here.'));
    assert.equal(v.length, 1);
    assert.match(v[0], /README/);
  });
});

describe('the repository itself', () => {
  const doc = readFileSync(join(root, 'docs', 'privacy.md'), 'utf8');

  it('has a parseable allowlist in docs/privacy.md', () => {
    const allow = parseAllowlist(doc);
    assert.ok(Array.isArray(allow.networkPermissions));
  });

  it('rejects a privacy statement without the allowlist block or with a malformed one', () => {
    assert.throws(() => parseAllowlist('# No list here'), /no allowlist block/);
    const broken = doc.replace('"networkPermissions": []', '"networkPermissions": "none"');
    assert.throws(() => parseAllowlist(broken), /networkPermissions/);
  });

  it('keeps the shipped configuration free of network permissions and remote origins', () => {
    const allow = parseAllowlist(doc);
    const conf = JSON.parse(
      readFileSync(join(root, 'apps/desktop/src-tauri/tauri.conf.json'), 'utf8'),
    );
    const capability = JSON.parse(
      readFileSync(join(root, 'apps/desktop/src-tauri/capabilities/default.json'), 'utf8'),
    );
    const reality = cleanReality();
    reality.tauriConf = conf;
    reality.capabilities = [{ path: 'capabilities/default.json', json: capability }];
    reality.crates = ['tauri', ...allow.tauriPlugins];
    reality.readme = readFileSync(join(root, 'README.md'), 'utf8');
    assert.deepEqual(runChecks(reality, allow), []);
  });
});
