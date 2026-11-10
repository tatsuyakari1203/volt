import { formatNumber } from '../i18n';

const UNITS = ['B', 'KB', 'MB', 'GB', 'TB'] as const;

const MIB = 1024 ** 2;
const GIB = 1024 ** 3;
const TIB = 1024 ** 4;

/** One decimal at most, without a trailing zero: 16 not 16.0, 9.9 stays 9.9. */
function trimmed(value: number): string {
  return formatNumber(value, { maximumFractionDigits: 1 });
}

/** Exactly one decimal, in the decimal mark of the current language. */
function oneDecimal(value: number): string {
  return formatNumber(value, { minimumFractionDigits: 1, maximumFractionDigits: 1 });
}

/**
 * A memory or disk size as the Windows shell writes it: binary units labelled GB, MB and TB.
 * Values of 100 or more are whole numbers, smaller ones keep one decimal. For the exact size of
 * files use [`formatSize`].
 */
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return '—';
  if (bytes >= TIB) return `${trimmed(bytes / TIB)} TB`;
  if (bytes >= GIB) {
    const gib = bytes / GIB;
    return `${gib >= 100 ? Math.round(gib) : trimmed(gib)} GB`;
  }
  return `${Math.round(bytes / MIB)} MB`;
}

/** A size the way the core writes it: powers of 1024, one decimal from KB up (`3.4 MB`). */
export function formatSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return '—';
  if (bytes < 1024) return `${Math.round(bytes)} B`;
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < UNITS.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${oneDecimal(value)} ${UNITS[unit]}`;
}
