import { getLocale, t, tn, tx } from '../../shared/i18n';
import type { CleaningRuleInfo, CleanRunResult, RuleScanView } from '../../shared/ipc';
import { formatSize } from '../../shared/text/size';

export { describeError } from '../../shared/text/errors';

/** The name of a rule. The core writes it in English; other languages are filed under its id. */
export function ruleTitle(id: string, english: string): string {
  return tx(`rule.${id}.title`, english);
}

/** What a rule takes and why that is safe, in the current language. */
export function ruleDescription(rule: CleaningRuleInfo): string {
  return tx(`rule.${rule.id}.description`, rule.description);
}

/** The name of the group a rule is listed under, in the current language. */
export function categoryName(rule: CleaningRuleInfo): string {
  return tx(`clean.category.${rule.category}`, rule.categoryLabel);
}

/** Groups whose cleaning has a visible cost afterwards, so the user chooses them. */
const UNTICKED_CATEGORIES: ReadonlySet<string> = new Set([
  'browserCache',
  'shaderCache',
  'chatAppCache',
  'developerCache',
]);

/**
 * Whether the rule is ticked when a scan finishes: Safe, no administrator rights needed, and not a
 * browser, shader or chat app cache (clearing it makes sites slower once, stutters games while
 * shaders compile again, makes a chat app download media again, or makes a package manager download packages again, so the user chooses it).
 */
export function tickedByDefault(rule: CleaningRuleInfo): boolean {
  return (
    rule.risk === 'safe' && !rule.needsAdministrator && !UNTICKED_CATEGORIES.has(rule.category)
  );
}

/** What a scan found for one rule, in one line. */
export function findingsLine(view: RuleScanView): string {
  if (view.problem) return view.problem;
  const parts: string[] = [];
  if (view.files > 0) {
    parts.push(
      t('clean.findings.filesSize', {
        files: tn('count.file', view.files),
        size: formatSize(view.bytes),
      }),
    );
  }
  if (view.folders > 0) parts.push(tn('count.folder', view.folders));
  const [first, second] = parts;
  if (first === undefined) return t('clean.findings.nothing');
  return second === undefined ? first : t('clean.findings.join', { first, second });
}

/**
 * The ticked rules, cleanable on this page, whose scan stopped at the limit or was cancelled: a
 * clean moves only a part of them, so the page offers to scan the rest once it is done.
 */
export function truncatedRuleIds(
  views: readonly RuleScanView[],
  ticked: ReadonlySet<string>,
): string[] {
  return views
    .filter((view) => view.truncated && ticked.has(view.rule.id) && !view.rule.needsAdministrator)
    .map((view) => view.rule.id);
}

/**
 * The ticked browser rules whose browser is running now: a clean skips the files a running browser
 * keeps open, and the confirmation says so.
 */
export function runningBrowserRules(
  views: readonly RuleScanView[],
  ticked: ReadonlySet<string>,
): RuleScanView[] {
  return views.filter(
    (view) =>
      ticked.has(view.rule.id) &&
      view.rule.category === 'browserCache' &&
      !view.rule.needsAdministrator &&
      view.lockedNote !== null,
  );
}

/** The totals of the ticked rules. */
export interface Selection {
  files: number;
  folders: number;
  bytes: number;
}

export function selectionOf(
  views: readonly RuleScanView[],
  ticked: ReadonlySet<string>,
): Selection {
  const total: Selection = { files: 0, folders: 0, bytes: 0 };
  for (const view of views) {
    if (!ticked.has(view.rule.id) || view.rule.needsAdministrator) continue;
    total.files += view.files;
    total.folders += view.folders;
    total.bytes += view.bytes;
  }
  return total;
}

/** What will be moved, for the summary and the confirmation. */
export function selectionText(selection: Selection): string {
  const files = tn('count.file', selection.files);
  const size = formatSize(selection.bytes);
  return selection.folders > 0
    ? t('clean.selection.withFolders', {
        files,
        folders: tn('count.folder', selection.folders),
        size,
      })
    : t('clean.selection.base', { files, size });
}

/**
 * What a clean did, in one or two sentences. English is the core's own sentence; another
 * language is put together from the totals the same result carries.
 */
export function resultMessage(result: CleanRunResult): string {
  if (getLocale() === 'en') return result.message;
  const files = tn('count.file', result.movedFiles);
  const moved =
    result.movedFolders > 0
      ? t('clean.findings.join', { first: files, second: tn('count.folder', result.movedFolders) })
      : files;
  return [
    t('clean.message.moved', { files: moved, size: formatSize(result.movedBytes) }),
    result.skippedTotal > 0
      ? t('clean.message.left', { items: tn('count.item', result.skippedTotal) })
      : '',
    t('clean.message.footer'),
  ]
    .filter(Boolean)
    .join(' ');
}
