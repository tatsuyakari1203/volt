const KIB = 1024;
const MIB = KIB * 1024;
const GIB = MIB * 1024;
const TIB = GIB * 1024;

/** One decimal, without a trailing `.0`: 16 not 16.0, 9.9 stays 9.9. */
function trimmed(value: number): string {
  return value.toFixed(1).replace(/\.0$/, '');
}

/**
 * A byte count as the Windows shell writes it: binary units labelled GB, MB and TB. Values of
 * 100 or more are whole numbers, smaller ones keep one decimal.
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

/** A 0 to 100 share as a whole percentage. */
export function formatPercent(value: number): string {
  return `${Math.round(value)}%`;
}

/** `C:\` as `C:`, the way a drive is named in a sentence. */
export function driveName(mount: string): string {
  return mount.replace(/\\+$/, '');
}
