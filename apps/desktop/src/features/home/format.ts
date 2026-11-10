/** A 0 to 100 share as a whole percentage. */
export function formatPercent(value: number): string {
  return `${Math.round(value)}%`;
}

/** `C:\` as `C:`, the way a drive is named in a sentence. */
export function driveName(mount: string): string {
  return mount.replace(/\\+$/, '');
}
