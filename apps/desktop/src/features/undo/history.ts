import { t } from '../../shared/i18n';
import type { ActivityItem } from '../../shared/ipc';
import { activityTitle } from './messages';

/** One top-level entry of the history list: a single row, or changes made together as a batch. */
export type HistoryEntry =
  | { kind: 'row'; item: ActivityItem }
  | { kind: 'group'; batch: string; title: string; items: ActivityItem[] };

function groupTitle(items: ActivityItem[]): string {
  return (
    items.map(activityTitle).find((title) => title) ??
    items.find((item) => item.label)?.label ??
    // Shown when a batch has no tweak title or label to name it by.
    t('undo.batch.unnamed')
  );
}

/**
 * Groups a newest-first list by batch: neighbouring records of the same journal batch (the
 * changes one applied plan made, and the undo of them) become one group. A record on its own, or
 * without a batch, stays a plain row. The order of the list never changes, and a group is never
 * formed from records that are not next to each other.
 */
export function groupHistory(items: readonly ActivityItem[]): HistoryEntry[] {
  const entries: HistoryEntry[] = [];
  for (const item of items) {
    const last = entries[entries.length - 1];
    if (item.batch !== null && last !== undefined) {
      if (last.kind === 'group' && last.batch === item.batch) {
        last.items.push(item);
        last.title = groupTitle(last.items);
        continue;
      }
      if (last.kind === 'row' && last.item.batch === item.batch) {
        const members = [last.item, item];
        entries[entries.length - 1] = {
          kind: 'group',
          batch: item.batch,
          title: groupTitle(members),
          items: members,
        };
        continue;
      }
    }
    entries.push({ kind: 'row', item });
  }
  return entries;
}
