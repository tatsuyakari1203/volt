import { formatNumber, t, tn } from '../../shared/i18n';
import type {
  EndAppResult,
  PriorityView,
  ProcessGroupView,
  TuneRequestView,
  TuneResult,
} from '../../shared/ipc';
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

/** The priority classes offered, lowest first. There is no Realtime: the core never offers it. */
export const PRIORITIES: readonly PriorityView[] = [
  'idle',
  'belowNormal',
  'normal',
  'aboveNormal',
  'high',
];

export function priorityLabel(priority: PriorityView): string {
  return t(`processes.tune.priority.${priority}`);
}

/** What a saved rule applies, in one line: `Low priority, 4 cores, Efficiency mode on`. */
export function ruleSummary(request: TuneRequestView): string {
  const parts: string[] = [];
  // Efficiency mode on means Low priority; it is said once, as the mode.
  if (request.priority && request.efficiency !== true) {
    parts.push(t('processes.rules.priority', { priority: priorityLabel(request.priority) }));
  }
  if (request.cores) parts.push(tn('count.core', request.cores.length));
  if (request.efficiency !== null) {
    parts.push(
      request.efficiency ? t('processes.rules.efficiencyOn') : t('processes.rules.efficiencyOff'),
    );
  }
  return parts.join(', ');
}

/** What a change did, in a few sentences. `problem` is true when something was left unchanged. */
export function tuneSummary(result: TuneResult): { text: string; problem: boolean } {
  const parts: string[] = [];
  parts.push(
    result.changed > 0
      ? t('processes.tune.done', {
          name: result.name,
          processes: processesText(result.changed),
        })
      : t('processes.tune.nothingChanged', { name: result.name }),
  );
  if (result.alreadySet > 0) {
    parts.push(t('processes.tune.alreadySet', { processes: processesText(result.alreadySet) }));
  }
  if (result.needsAdministrator > 0) {
    parts.push(
      t('processes.tune.leftAlone', { processes: processesText(result.needsAdministrator) }),
    );
  }
  if (result.skipped > 0) {
    parts.push(t('processes.tune.skipped', { processes: processesText(result.skipped) }));
  }
  if (result.ruleSaved) parts.push(t('processes.tune.ruleSaved', { name: result.name }));
  if (!result.recorded) parts.push(t('processes.tune.notRecorded'));
  const problem = result.needsAdministrator > 0 || result.skipped > 0 || !result.recorded;
  return { text: parts.join(' '), problem };
}
