import { localeTag, t, tn } from '../../shared/i18n';
import type { FileGroupView, LargeFilesFilter, LargeFileView } from '../../shared/ipc';
import { formatSize } from '../../shared/text/size';

const DAY_MS = 86_400_000;

/** The type filter: one group, or all of them. */
export type GroupFilter = FileGroupView | 'all';

/** The age filter: only files not changed for this many days, or `0` for any age. */
export type AgeFilter = 0 | 30 | 90 | 365;

export const GROUP_FILTERS: readonly GroupFilter[] = [
  'all',
  'video',
  'archive',
  'diskImage',
  'installer',
  'other',
];

export const AGE_FILTERS: readonly AgeFilter[] = [0, 30, 90, 365];

/** The file name and the folder it is in, split at the last separator. */
export function splitPath(path: string): { name: string; folder: string } {
  const cut = Math.max(path.lastIndexOf('\\'), path.lastIndexOf('/'));
  return cut < 0
    ? { name: path, folder: '' }
    : { name: path.slice(cut + 1), folder: path.slice(0, cut) };
}

/** The filter the scan is asked to apply while it walks the folder; `null` when there is none. */
export function scanFilter(group: GroupFilter, age: AgeFilter): LargeFilesFilter | null {
  if (group === 'all' && age === 0) return null;
  return { group: group === 'all' ? null : group, minAgeDays: age === 0 ? null : age };
}

/** The listed files that pass both filters, in the order the scan gave (biggest first). */
export function visibleFiles(
  files: readonly LargeFileView[],
  group: GroupFilter,
  age: AgeFilter,
  scannedAtMs: number,
): LargeFileView[] {
  const cutoff = scannedAtMs - age * DAY_MS;
  return files.filter(
    (file) =>
      (group === 'all' || file.group === group) &&
      (age === 0 || (file.modifiedMs !== null && file.modifiedMs <= cutoff)),
  );
}

/** The ticked files among those on screen: a ticked file a filter hides is never moved. */
export function selectedFiles(
  visible: readonly LargeFileView[],
  ticked: ReadonlySet<number>,
): LargeFileView[] {
  return visible.filter((file) => ticked.has(file.index));
}

/** What will be moved, for the summary and the confirmation: `3 files (4.2 GB)`. */
export function selectionLabel(files: readonly LargeFileView[]): string {
  const bytes = files.reduce((sum, file) => sum + file.sizeBytes, 0);
  return t('large.selection.base', {
    files: tn('count.file', files.length),
    size: formatSize(bytes),
  });
}

/** A modification time as a date in the current language. */
export function formatDate(ms: number): string {
  return new Date(ms).toLocaleDateString(localeTag(), {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
  });
}
