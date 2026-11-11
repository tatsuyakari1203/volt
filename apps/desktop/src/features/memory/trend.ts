import type { MemoryListsView, MemoryPointView } from '../../shared/ipc';

/** How much time the chart spans: the same five minutes the shell keeps. */
export const SPAN_MS = 5 * 60 * 1000;

/** The most readings kept in the UI (one a second for the span). */
export const MAX_POINTS = 300;

/** The four lists the chart stacks, bottom to top. */
export const LIST_KEYS = ['inUseBytes', 'standbyBytes', 'modifiedBytes', 'freeBytes'] as const;
export type ListKey = (typeof LIST_KEYS)[number];

/** Adds `incoming` to `current`: sorted by time, no duplicates, at most `MAX_POINTS` newest. */
export function mergePoints(
  current: readonly MemoryPointView[],
  incoming: readonly MemoryPointView[],
): MemoryPointView[] {
  const byTime = new Map<string, MemoryPointView>();
  for (const point of current) byTime.set(point.takenAt, point);
  for (const point of incoming) byTime.set(point.takenAt, point);
  // ISO 8601 in UTC with a fixed width sorts as text.
  const sorted = [...byTime.values()].sort((a, b) => (a.takenAt < b.takenAt ? -1 : 1));
  return sorted.slice(-MAX_POINTS);
}

/** One drawn layer: which list, and the polygon outline in the chart's 0..width, 0..height box. */
export interface Layer {
  key: ListKey;
  /** SVG path data of the filled area. */
  area: string;
  /** SVG path data of its top edge, drawn as a line. */
  edge: string;
}

function at(point: MemoryPointView): number {
  return Date.parse(point.takenAt);
}

function round(value: number): string {
  return (Math.round(value * 100) / 100).toString();
}

/**
 * The stacked layers for `points` in a `width` by `height` box. The horizontal axis is time: the
 * newest reading sits at the right edge and the box spans `SPAN_MS`, so a short history fills only
 * the right part. The vertical axis is 0 to the installed memory. Fewer than two readings draw
 * nothing.
 */
export function buildLayers(
  points: readonly MemoryPointView[],
  width: number,
  height: number,
): Layer[] {
  const last = points[points.length - 1];
  if (!last || points.length < 2 || last.lists.totalBytes <= 0) return [];
  const end = at(last);
  const total = last.lists.totalBytes;
  const xs = points.map((point) => ((at(point) - (end - SPAN_MS)) / SPAN_MS) * width);
  const y = (bytes: number) => height - Math.min(1, Math.max(0, bytes / total)) * height;
  const floors = points.map(() => 0);
  return LIST_KEYS.map((key) => {
    const tops = points.map((point, index) => (floors[index] ?? 0) + point.lists[key]);
    const topPoints = tops.map((top, index) => `${round(xs[index] ?? 0)},${round(y(top))}`);
    const bottomPoints = points
      .map((_, index) => `${round(xs[index] ?? 0)},${round(y(floors[index] ?? 0))}`)
      .reverse();
    for (const [index, top] of tops.entries()) floors[index] = top;
    return {
      key,
      area: `M${topPoints.join(' L')} L${bottomPoints.join(' L')} Z`,
      edge: `M${topPoints.join(' L')}`,
    };
  });
}

/** The lowest and highest value of one list over the readings. */
export function rangeOf(points: readonly MemoryPointView[], key: keyof MemoryListsView) {
  let low = Number.POSITIVE_INFINITY;
  let high = Number.NEGATIVE_INFINITY;
  for (const point of points) {
    low = Math.min(low, point.lists[key]);
    high = Math.max(high, point.lists[key]);
  }
  return { low, high };
}

/** How many whole minutes the readings cover, at least 1. */
export function coveredMinutes(points: readonly MemoryPointView[]): number {
  const first = points[0];
  const last = points[points.length - 1];
  if (!first || !last) return 0;
  return Math.max(1, Math.round((at(last) - at(first)) / 60_000));
}
