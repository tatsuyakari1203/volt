---
id: TASK-030
title: Cleaning rules for developer and AI caches
status: To Do
assignee: []
created_date: '2026-11-10T03:01:00Z'
labels:
  - cleaner
milestone: m-2
dependencies:
  - TASK-016
priority: medium
type: feature
ordinal: 30000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Developer and local-AI machines lose tens of gigabytes to package and model caches. The owner's machine has exactly this workload. Matrix rows C7 and C8.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 npm, pnpm, pip, cargo, NuGet, Gradle and Docker caches are covered, using each tool's own clean command where one exists
- [ ] #2 Hugging Face, torch hub and ComfyUI temp and output folders are covered; model files are listed individually and never selected by default
- [ ] #3 Tools that are not installed are skipped silently
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
