import { localeTag, t } from '../../shared/i18n';
import type {
  AppProblemView,
  AppSourceView,
  AppView,
  HiddenReasonView,
  UninstallOutcomeView,
} from '../../shared/ipc';
import { formatSize } from '../../shared/text/size';

/** How the list is ordered. Name is A to Z; size and date put the biggest and newest first. */
export type SortKey = 'name' | 'size' | 'date';

/** What happened to one app of the queue, or what it is waiting to do. */
export type ItemStatus =
  | 'waiting'
  | 'running'
  | 'removed'
  | 'stillInstalled'
  | 'failed'
  | 'unknown'
  | 'gone'
  | 'skipped';

function byName(a: AppView, b: AppView): number {
  return (
    a.name.localeCompare(b.name, undefined, { sensitivity: 'base' }) || a.id.localeCompare(b.id)
  );
}

/** Biggest first; an app with no known value goes last. */
function byValueDescending(value: (app: AppView) => number | string | null) {
  return (a: AppView, b: AppView): number => {
    const left = value(a);
    const right = value(b);
    if (left === null && right === null) return byName(a, b);
    if (left === null) return 1;
    if (right === null) return -1;
    if (left === right) return byName(a, b);
    return left < right ? 1 : -1;
  };
}

const comparers: Record<SortKey, (a: AppView, b: AppView) => number> = {
  name: byName,
  size: byValueDescending((app) => app.sizeBytes),
  date: byValueDescending((app) => app.installed),
};

/**
 * The apps to draw: the ones that match the search (name or publisher), without the hidden ones
 * unless system components are shown, in the chosen order.
 */
export function visibleApps(
  apps: readonly AppView[],
  query: string,
  sort: SortKey,
  showSystem: boolean,
): AppView[] {
  const needle = query.trim().toLowerCase();
  return apps
    .filter((app) => showSystem || app.hidden === null)
    .filter((app) => !needle || `${app.name} ${app.publisher ?? ''}`.toLowerCase().includes(needle))
    .sort(comparers[sort]);
}

/** How many apps the default view leaves out. */
export function hiddenCount(apps: readonly AppView[]): number {
  return apps.filter((app) => app.hidden !== null).length;
}

/** The apps that are ticked, still listed, removable and not hidden by the current view. */
export function selectedApps(
  apps: readonly AppView[],
  ticked: ReadonlySet<string>,
  showSystem: boolean,
): AppView[] {
  return apps.filter(
    (app) => ticked.has(app.id) && app.uninstallable && (showSystem || app.hidden === null),
  );
}

/** `2024-02-29` as a date in the language of the UI. */
export function dateText(iso: string): string {
  const [year, month, day] = iso.split('-').map(Number);
  if (!year || !month || !day) return iso;
  return new Date(year, month - 1, day).toLocaleDateString(localeTag(), {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
  });
}

/** The details line of a row: version, size and install date, whichever are known. */
export function detailsText(app: AppView): string {
  const parts: string[] = [];
  if (app.version) parts.push(t('apps.row.version', { version: app.version }));
  if (app.sizeBytes !== null) parts.push(formatSize(app.sizeBytes));
  if (app.installed) parts.push(t('apps.row.installed', { date: dateText(app.installed) }));
  return parts.join(' · ');
}

const hiddenKeys = {
  systemComponent: 'apps.hidden.systemComponent',
  update: 'apps.hidden.update',
  framework: 'apps.hidden.framework',
  notRemovable: 'apps.hidden.notRemovable',
  noUninstallCommand: 'apps.hidden.noUninstallCommand',
} as const satisfies Record<HiddenReasonView, string>;

/** Why an app is hidden by default, in words. */
export function hiddenText(reason: HiddenReasonView): string {
  return t(hiddenKeys[reason]);
}

const sourceKeys = {
  machine64: 'apps.problem.source.machine64',
  machine32: 'apps.problem.source.machine32',
  currentUser: 'apps.problem.source.currentUser',
  packaged: 'apps.problem.source.packaged',
} as const satisfies Record<AppSourceView, string>;

/** One sentence for each source that could not be read completely. */
export function problemText(problem: AppProblemView, entries: (count: number) => string): string {
  const source = t(sourceKeys[problem.source]);
  return problem.unreadable
    ? t('apps.problem.unreadable', { source })
    : t('apps.problem.skipped', { entries: entries(problem.skipped), source });
}

/** The status a finished uninstall puts its queue item in. */
export function statusOf(outcome: UninstallOutcomeView): ItemStatus {
  switch (outcome) {
    case 'removed':
      return 'removed';
    case 'stillInstalled':
      return 'stillInstalled';
    case 'unknown':
      return 'unknown';
    case 'failed':
      return 'failed';
  }
}

/** Counts of the finished queue by how each item ended. */
export function tally(items: ReadonlyArray<{ status: ItemStatus }>) {
  const count = (...wanted: ItemStatus[]) =>
    items.filter((item) => wanted.includes(item.status)).length;
  return {
    removed: count('removed', 'gone'),
    still: count('stillInstalled', 'unknown'),
    failed: count('failed'),
    skipped: count('skipped'),
    finished: count('removed', 'gone', 'stillInstalled', 'unknown', 'failed'),
  };
}
