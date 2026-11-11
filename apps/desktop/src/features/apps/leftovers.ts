import { type MessageKey, say, t } from '../../shared/i18n';
import type { ConfidenceView, LeftoverScanView, LeftoverView } from '../../shared/ipc';

const confidenceKeys = {
  high: 'apps.leftovers.confidence.high',
  medium: 'apps.leftovers.confidence.medium',
  low: 'apps.leftovers.confidence.low',
} as const satisfies Record<ConfidenceView, MessageKey>;

/** The words for a confidence level. */
export function confidenceText(confidence: ConfidenceView): string {
  return t(confidenceKeys[confidence]);
}

/** Why Volt thinks a find belongs to the app, one sentence per piece of evidence. */
export function evidenceTexts(item: LeftoverView): string[] {
  return item.evidence.map((english, at) => say(item.evidencePhrases[at], english));
}

/** Why a find cannot be removed from here, or `null` when it can. */
export function lockedText(item: LeftoverView): string | null {
  return item.locked === null ? null : say(item.lockedPhrase, item.locked);
}

/** The ids the page ticks at first: high confidence and not locked. */
export function defaultSelection(scan: LeftoverScanView): Set<string> {
  return new Set(
    scan.items.filter((item) => item.selectedByDefault && item.locked === null).map((i) => i.id),
  );
}

/** The finds of `scan` that are ticked and can be removed, in the order of the scan. */
export function chosenItems(scan: LeftoverScanView, ticked: ReadonlySet<string>): LeftoverView[] {
  return scan.items.filter((item) => ticked.has(item.id) && item.locked === null);
}

/** The bytes of the folders among `items`. */
export function totalBytes(items: readonly LeftoverView[]): number {
  return items.reduce((sum, item) => sum + item.sizeBytes, 0);
}
