#!/usr/bin/env node
// Enforces the crate layering described in the Architecture doc (.backlog/docs).
//
//   volt-desktop, volt-cli   composition roots: wire adapters into the core
//        |          |
//   volt-win    volt-store   adapters: implement the core's port traits
//        \          /
//         volt-core          domain, use cases, ports: depends on nothing internal
//
// Only DIRECT dependencies are checked: the point is to stop a layer from
// coupling itself to the wrong thing, not to audit the whole tree.
// A workspace crate without an entry here fails the check on purpose: adding a
// crate is an architecture decision, so record it here (and in a decision).

import { execFileSync } from 'node:child_process';

/** @type {Record<string, { internal: string[], banned: string[] }>} */
const RULES = {
  'volt-core': {
    internal: [],
    banned: ['tauri', 'windows', 'windows-sys', 'winreg', 'rusqlite'],
  },
  'volt-win': {
    internal: ['volt-core'],
    banned: ['tauri', 'rusqlite'],
  },
  'volt-store': {
    internal: ['volt-core'],
    banned: ['tauri', 'windows', 'windows-sys', 'winreg'],
  },
  'volt-desktop': {
    internal: ['volt-core', 'volt-win', 'volt-store'],
    banned: [],
  },
  'volt-cli': {
    internal: ['volt-core', 'volt-win', 'volt-store'],
    banned: ['tauri'],
  },
};

/** `tauri` also bans `tauri-build`, `tauri-plugin-*`; `windows` bans `windows-core` etc. */
const matchesBan = (dep, ban) => dep === ban || dep.startsWith(`${ban}-`);

const metadata = JSON.parse(
  execFileSync('cargo', ['metadata', '--format-version', '1', '--no-deps'], {
    encoding: 'utf8',
    maxBuffer: 64 * 1024 * 1024,
  }),
);

const members = new Set(metadata.packages.map((p) => p.name));
const violations = [];

for (const pkg of metadata.packages) {
  const rule = RULES[pkg.name];
  if (!rule) {
    violations.push(`${pkg.name}: no layering rule. Add it to RULES in scripts/check-arch.mjs.`);
    continue;
  }
  for (const dep of pkg.dependencies) {
    const kind = dep.kind ?? 'normal';
    if (members.has(dep.name)) {
      if (!rule.internal.includes(dep.name)) {
        violations.push(
          `${pkg.name} -> ${dep.name} (${kind}): not an allowed internal dependency. Allowed: ${
            rule.internal.join(', ') || 'none'
          }.`,
        );
      }
      continue;
    }
    const ban = rule.banned.find((b) => matchesBan(dep.name, b));
    if (ban) {
      violations.push(
        `${pkg.name} -> ${dep.name} (${kind}): banned in this layer (rule "${ban}").`,
      );
    }
  }
}

if (violations.length > 0) {
  console.error(`Architecture check failed (${violations.length}):`);
  for (const v of violations) console.error(`  - ${v}`);
  process.exit(1);
}

console.log(`Architecture check passed: ${metadata.packages.length} crates, layering respected.`);
