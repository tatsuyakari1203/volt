import { formatNumber, t, tn } from '../../shared/i18n';
import type { EndAppResult, ProcessGroupView } from '../../shared/ipc';
import { formatSize } from '../../shared/text/size';

export type SortKey = 'memory' | 'cpu';

/** The apps that match `query` (a name fragment, any case), biggest first by the chosen figure. */
export function visibleGroups(
  groups: readonly ProcessGroupView[],
  query: string,
  sort: SortKey,
): ProcessGroupView[] {
  const needle = query.trim().toLowerCase();
  const matching = needle ? groups.filter((group) => group.id.includes(needle)) : [...groups];
  return matching.sort((a, b) => {
    const byCpu = sort === 'cpu' ? (b.cpuPercent ?? 0) - (a.cpuPercent ?? 0) : 0;
    if (byCpu !== 0) return byCpu;
    // Memory is the default order and the tie-break, then the name, so equal rows never swap.
    return b.memoryBytes - a.memoryBytes || a.id.localeCompare(b.id);
  });
}

/** CPU share as shown in a row: `–` before there is a second reading, one decimal under 10. */
export function cpuText(percent: number | null): string {
  if (percent === null) return '–';
  if (percent < 0.05) return '0%';
  return percent < 10
    ? `${formatNumber(percent, { minimumFractionDigits: 1, maximumFractionDigits: 1 })}%`
    : `${Math.round(percent)}%`;
}

/** Memory as shown in a row. */
export function memoryText(bytes: number): string {
  return formatSize(bytes);
}

export function processesText(count: number): string {
  return tn('count.process', count);
}

/** What an end-task run did, in one or two sentences. `problem` is true when anything was left. */
export function endSummary(result: EndAppResult): { text: string; problem: boolean } {
  const parts: string[] = [];
  parts.push(
    result.ended > 0
      ? t('processes.end.ended', { name: result.name, processes: processesText(result.ended) })
      : t('processes.end.nothing', { name: result.name }),
  );
  if (result.needsAdministrator > 0) {
    parts.push(
      t('processes.end.needsAdmin', { processes: processesText(result.needsAdministrator) }),
    );
  }
  if (result.failed > 0) {
    parts.push(t('processes.end.failed', { processes: processesText(result.failed) }));
  }
  if (result.alreadyGone > 0 && result.ended === 0) {
    parts.push(t('processes.end.alreadyClosed'));
  }
  if (!result.recorded) {
    parts.push(t('processes.end.notRecorded'));
  }
  const problem = result.needsAdministrator > 0 || result.failed > 0 || !result.recorded;
  return { text: parts.join(' '), problem };
}
