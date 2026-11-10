import {
  type CleaningRuleInfo,
  describeIpcError,
  IpcCommandError,
  IpcUnavailableError,
  type RuleScanView,
} from '../../shared/ipc';
import { countOf, formatSize } from './format';

/** One sentence for any error that reached the UI. */
export function describeError(error: unknown): string {
  if (error instanceof IpcUnavailableError) {
    return 'This is only available inside the Volt app.';
  }
  if (error instanceof IpcCommandError) {
    return describeIpcError(error.error);
  }
  return error instanceof Error ? error.message : String(error);
}

/** Whether the rule is ticked when a scan finishes: Safe, and no administrator rights needed. */
export function tickedByDefault(rule: CleaningRuleInfo): boolean {
  return rule.risk === 'safe' && !rule.needsAdministrator;
}

/** What a scan found for one rule, in one line. */
export function findingsLine(view: RuleScanView): string {
  if (view.problem) return view.problem;
  const parts: string[] = [];
  if (view.files > 0) parts.push(`${countOf(view.files, 'file')}, ${formatSize(view.bytes)}`);
  if (view.folders > 0) parts.push(countOf(view.folders, 'empty folder'));
  if (parts.length === 0) return 'Nothing to clean';
  return parts.join(' and ');
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
  const folders = selection.folders > 0 ? ` and ${countOf(selection.folders, 'empty folder')}` : '';
  return `${countOf(selection.files, 'file')}${folders} (${formatSize(selection.bytes)})`;
}
