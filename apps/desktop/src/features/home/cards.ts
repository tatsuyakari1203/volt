import { t } from '../../shared/i18n';
import type { MetricsView } from '../../shared/ipc';
import { formatBytes } from '../../shared/text/size';
import { driveName, formatPercent } from './format';

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
    ? {
        id: 'cpu',
        label: t('home.card.cpu'),
        value: NO_VALUE,
        lines: [t('home.card.cpu.none')],
        empty: true,
      }
    : {
        id: 'cpu',
        label: t('home.card.cpu'),
        value: formatPercent(cpu),
        lines: [t('home.card.cpu.busy')],
        empty: false,
      };
}

function memoryCard(metrics: MetricsView): MetricCardSpec {
  const { memory } = metrics;
  return {
    id: 'memory',
    label: t('home.card.memory'),
    value: formatPercent(memory.usedPercent),
    lines: [
      t('home.card.memory.inUse', {
        used: formatBytes(memory.usedBytes),
        total: formatBytes(memory.totalBytes),
      }),
    ],
    empty: false,
  };
}

function gpuCard(metrics: MetricsView): MetricCardSpec {
  const { gpu } = metrics;
  if (gpu === null) {
    return {
      id: 'gpu',
      label: t('home.card.gpu'),
      value: NO_VALUE,
      lines: [t('home.card.gpu.none')],
      empty: true,
    };
  }
  const lines = [gpu.name];
  if (gpu.vramUsedBytes !== null && gpu.vramTotalBytes !== null) {
    lines.push(
      t('home.card.gpu.vram', {
        used: formatBytes(gpu.vramUsedBytes),
        total: formatBytes(gpu.vramTotalBytes),
      }),
    );
  }
  if (gpu.utilisationPercent === null) {
    lines.push(t('home.card.gpu.noUsage'));
    return { id: 'gpu', label: t('home.card.gpu'), value: NO_VALUE, lines, empty: true };
  }
  return {
    id: 'gpu',
    label: t('home.card.gpu'),
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
      label: t('home.card.disk'),
      value: NO_VALUE,
      lines: [t('home.card.disk.none')],
      empty: true,
    };
  }
  return {
    id: 'disk',
    label: t('home.card.disk'),
    value: formatBytes(disk.freeBytes),
    lines: [
      t('home.card.disk.free', {
        total: formatBytes(disk.totalBytes),
        drive: driveName(disk.mount),
      }),
    ],
    empty: false,
  };
}
