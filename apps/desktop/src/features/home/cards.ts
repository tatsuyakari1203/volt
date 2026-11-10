import type { MetricsView } from '../../shared/ipc';
import { driveName, formatBytes, formatPercent } from './format';

/** The four live cards, in the order they are shown. */
export type MetricCardId = 'cpu' | 'memory' | 'gpu' | 'disk';

/** What one live card shows. The UI renders it as it is; nothing here is invented. */
export interface MetricCardSpec {
  id: MetricCardId;
  label: string;
  /** The big figure. A dash when there is no number. */
  value: string;
  /** Supporting lines under the figure. */
  lines: string[];
  /** True when there is no figure to show (a `null` from the shell, or no such device). */
  empty: boolean;
}

/** The value shown when there is no number. Never a placeholder digit. */
export const NO_VALUE = '—';

/**
 * Turns one reading into the four cards. A value the shell reported as `null` becomes an empty
 * card with a plain explanation, never a made-up number.
 *
 * `systemMount` (such as `C:\`) picks the disk for the disk card; without it, or when that disk
 * is not in the reading, the first disk is shown and named, so the card never claims a drive it
 * is not showing.
 */
export function buildCards(metrics: MetricsView, systemMount?: string): MetricCardSpec[] {
  return [cpuCard(metrics), memoryCard(metrics), gpuCard(metrics), diskCard(metrics, systemMount)];
}

function cpuCard(metrics: MetricsView): MetricCardSpec {
  const cpu = metrics.cpuPercent;
  return cpu === null
    ? { id: 'cpu', label: 'CPU', value: NO_VALUE, lines: ['No reading yet'], empty: true }
    : {
        id: 'cpu',
        label: 'CPU',
        value: formatPercent(cpu),
        lines: ['Busy right now'],
        empty: false,
      };
}

function memoryCard(metrics: MetricsView): MetricCardSpec {
  const { memory } = metrics;
  return {
    id: 'memory',
    label: 'Memory',
    value: formatPercent(memory.usedPercent),
    lines: [`${formatBytes(memory.usedBytes)} of ${formatBytes(memory.totalBytes)} in use`],
    empty: false,
  };
}

function gpuCard(metrics: MetricsView): MetricCardSpec {
  const { gpu } = metrics;
  if (gpu === null) {
    return { id: 'gpu', label: 'GPU', value: NO_VALUE, lines: ['No GPU detected'], empty: true };
  }
  const lines = [gpu.name];
  if (gpu.vramUsedBytes !== null && gpu.vramTotalBytes !== null) {
    lines.push(
      `${formatBytes(gpu.vramUsedBytes)} of ${formatBytes(gpu.vramTotalBytes)} video memory`,
    );
  }
  if (gpu.utilisationPercent === null) {
    lines.push('Usage is not available');
    return { id: 'gpu', label: 'GPU', value: NO_VALUE, lines, empty: true };
  }
  return {
    id: 'gpu',
    label: 'GPU',
    value: formatPercent(gpu.utilisationPercent),
    lines,
    empty: false,
  };
}

function diskCard(metrics: MetricsView, systemMount?: string): MetricCardSpec {
  const wanted = systemMount ? driveName(systemMount).toLowerCase() : null;
  const disk =
    metrics.disks.find((candidate) => driveName(candidate.mount).toLowerCase() === wanted) ??
    metrics.disks[0];
  if (!disk) {
    return {
      id: 'disk',
      label: 'Disk space',
      value: NO_VALUE,
      lines: ['No disk reading'],
      empty: true,
    };
  }
  return {
    id: 'disk',
    label: 'Disk space',
    value: formatBytes(disk.freeBytes),
    lines: [`Free of ${formatBytes(disk.totalBytes)} on ${driveName(disk.mount)}`],
    empty: false,
  };
}
