<!-- Title must be a Conventional Commit, e.g. feat(core): add undo journal entries -->

## What and why

<!-- What changes, and what problem it solves. Link the task: Refs: TASK-NNN -->

## How it was verified

- [ ] `pnpm check` passes
- [ ] New behaviour has tests (OS-touching logic tested through fakes)

## System-changing code (delete if not applicable)

- [ ] Has detect, apply and revert, and revert is tested
- [ ] Records previous values in the undo journal before applying
- [ ] Supports dry-run / preview
- [ ] Risk level and evidence are stated

## Docs

- [ ] Task updated (acceptance criteria, notes, final summary)
- [ ] Feature matrix, docs or a decision updated if behaviour or a decision changed
