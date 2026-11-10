---
id: TASK-037
title: Tweak packs for gaming, network, Windows Update
status: To Do
assignee: []
created_date: '2026-11-10T03:01:00Z'
labels:
  - tweaks
milestone: m-2
dependencies:
  - TASK-022
priority: medium
type: feature
ordinal: 37000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
These categories carry the most folklore. Each tweak needs an evidence note so Volt does not ship placebo. Matrix rows E2, E5 and E6.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Gaming, Network and Windows Update tweaks are defined as data with risk and evidence
- [ ] #2 Tweaks with no measurable effect in testing are left out or labelled Anecdotal
- [ ] #3 DNS switching offers presets and custom servers and is reversible
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
