const UNITS = ['B', 'KB', 'MB', 'GB', 'TB'] as const;

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
  return `${value.toFixed(1)} ${UNITS[unit]}`;
}

/** `1 file`, `2 files`; the count is grouped (`12,345 files`). */
export function countOf(count: number, one: string, many = `${one}s`): string {
  return `${count.toLocaleString('en-US')} ${count === 1 ? one : many}`;
}
