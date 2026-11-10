---
id: decision-007
title: Open-source license
date: '2026-11-10T03:01:00Z'
status: proposed
---
## Context

The repository is public on GitHub under the owner's personal account. Without a license file nobody may legally use or contribute to the code. The realistic options are a permissive license (MIT or Apache-2.0) or a copyleft license (GPL-3.0-or-later). Optimizer software is frequently repackaged with adware by third parties.

## Decision

Proposed: GPL-3.0-or-later, so that redistributed and modified versions must stay open. Awaiting the owner's confirmation; until then the repo has no LICENSE file and grants no license.

## Consequences

- GPL discourages closed, ad-bundled forks; it does not prevent the owner from dual-licensing later, provided contributions are handled accordingly.
- MIT would maximise reuse but allows closed repackaging.
- Dependencies must be GPL-compatible; `deny.toml` already restricts licenses to permissive ones.
- Once chosen: add LICENSE, set the `license` field in `Cargo.toml` and `package.json`, and update the README.
