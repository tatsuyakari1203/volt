import { t, tn } from '../../shared/i18n';
import type { DuplicateGroupView, TidyResult } from '../../shared/ipc';
import { formatSize } from '../../shared/text/size';

/** The copies of each duplicate group the user marked for removal: group row to copy numbers. */
export type Marks = ReadonlyMap<number, ReadonlySet<number>>;

/** Marks or unmarks one copy. A group with no mark has no entry. */
export function withMark(marks: Marks, group: number, file: number, on: boolean): Marks {
  const next = new Map(marks);
  const copies = new Set(marks.get(group));
  if (on) copies.add(file);
  else copies.delete(file);
  if (copies.size > 0) next.set(group, copies);
  else next.delete(group);
  return next;
}

/**
 * Keeps one copy: marks every other copy of the group for removal and clears the mark on the kept
 * one. Nothing is marked until the user does this or ticks a copy.
 */
export function keepOnly(marks: Marks, group: DuplicateGroupView, keep: number): Marks {
  const next = new Map(marks);
  const others = new Set(group.files.map((_, file) => file).filter((file) => file !== keep));
  if (others.size > 0) next.set(group.index, others);
  else next.delete(group.index);
  return next;
}

/**
 * Whether ticking this copy is allowed: the last copy that is not marked stays, so a group can
 * never have every copy marked.
 */
export function canMark(marks: Marks, group: DuplicateGroupView, marked: boolean): boolean {
  if (marked) return true;
  return group.files.length - (marks.get(group.index)?.size ?? 0) > 1;
}

/** The marked copies in the form the move takes: a group row and the numbers of its copies. */
export function picksOf(marks: Marks): { group: number; files: number[] }[] {
  return [...marks.entries()]
    .map(([group, files]) => ({ group, files: [...files].sort((a, b) => a - b) }))
    .sort((a, b) => a.group - b.group);
}

/** How many copies are marked and how much space they take. */
export function markedTotals(
  groups: readonly DuplicateGroupView[],
  marks: Marks,
): { copies: number; bytes: number } {
  let copies = 0;
  let bytes = 0;
  for (const group of groups) {
    const count = marks.get(group.index)?.size ?? 0;
    copies += count;
    bytes += count * group.sizeBytes;
  }
  return { copies, bytes };
}

/** Drops marks that point at a group or copy a newer list no longer has. */
export function pruneMarks(marks: Marks, groups: readonly DuplicateGroupView[]): Marks {
  const next = new Map<number, ReadonlySet<number>>();
  for (const group of groups) {
    const kept = [...(marks.get(group.index) ?? [])].filter((file) => file < group.files.length);
    if (kept.length > 0 && kept.length < group.files.length) next.set(group.index, new Set(kept));
  }
  return next;
}

/** `3 files (4.2 GB)`. */
export function copiesLabel(copies: number, bytes: number): string {
  return t('tidy.selection.sized', { items: tn('count.file', copies), size: formatSize(bytes) });
}

/** The notes under a scan summary: what was left out or could not be judged. */
export function scanNotes(scan: TidyResult): string[] {
  const notes: string[] = [];
  if (scan.unreadable > 0) notes.push(t('tidy.note.unreadable', { count: scan.unreadable }));
  if (scan.links > 0) notes.push(t('tidy.note.links', { count: scan.links }));
  if (scan.cloudSkipped > 0) notes.push(t('tidy.note.cloud', { count: scan.cloudSkipped }));
  if (scan.hardLinksFolded > 0)
    notes.push(t('tidy.note.hardLinks', { count: scan.hardLinksFolded }));
  if (scan.shortcutsUnchecked > 0)
    notes.push(t('tidy.note.shortcuts', { count: scan.shortcutsUnchecked }));
  if (scan.groupsOmitted > 0) notes.push(t('tidy.note.omitted', { count: scan.groupsOmitted }));
  if (scan.truncated) notes.push(t('tidy.note.truncated'));
  return notes;
}
