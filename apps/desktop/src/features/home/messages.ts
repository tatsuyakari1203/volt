import { getLocale, type MessageKey, say, t, tn } from '../../shared/i18n';
import type {
  DeductionView,
  FixActionView,
  HealthCheckView,
  HealthLevelView,
  HealthView,
} from '../../shared/ipc';

export { describeError } from '../../shared/text/errors';

/** The sections Home can send the user to. A subset of the shell's sections. */
export type HomeTarget = 'clean' | 'memory' | 'startup';

/** Where a fix leads and what its button says. */
export interface FixLink {
  target: HomeTarget;
  label: string;
}

/**
 * The button for a fix, or `null` when the fix is a sentence and not a screen (Restart Windows:
 * Volt has no restart button, so it never shows one) or the fix is not known to this UI.
 * The switch is exhaustive: a new Rust variant fails the typecheck here.
 */
export function fixLink(fix: FixActionView | null): FixLink | null {
  if (fix === null) return null;
  switch (fix) {
    case 'openCleaner':
      return { target: 'clean', label: t('home.fix.openCleaner') };
    case 'openMemory':
      return { target: 'memory', label: t('home.fix.openMemory') };
    case 'openStartup':
      return { target: 'startup', label: t('home.fix.openStartup') };
    case 'restartWindows':
      return null;
    default:
      return assertNever(fix);
  }
}

/** A sentence that stands in for the button of a fix that has none. */
export function fixHint(fix: FixActionView | null): string | null {
  return fix === 'restartWindows' ? t('home.fix.restartHint') : null;
}

/** The name of a check, for the line that says which checks could not run. */
export function checkName(check: HealthCheckView): string {
  switch (check) {
    case 'systemDisk':
      return t('home.check.systemDisk');
    case 'memory':
      return t('home.check.memory');
    case 'pendingRestart':
      return t('home.check.pendingRestart');
    case 'startup':
      return t('home.check.startup');
    default:
      return assertNever(check);
  }
}

/** The word under the score. */
export function levelLabel(level: HealthLevelView): string {
  switch (level) {
    case 'good':
      return t('home.level.good');
    case 'fair':
      return t('home.level.fair');
    case 'poor':
      return t('home.level.poor');
    case 'unknown':
      return t('home.level.unknown');
    default:
      return assertNever(level);
  }
}

/** The headline and the sentence under it, from the grade and the number of deductions. */
export function headline(report: HealthView): { title: string; detail: string } {
  const count = report.deductions.length;
  const lowering = count === 0 ? t('home.lowering.none') : tn('home.lowering', count);
  switch (report.level) {
    case 'good':
      return { title: t('home.headline.good'), detail: lowering };
    case 'fair':
      return { title: t('home.headline.fair'), detail: lowering };
    case 'poor':
      return { title: t('home.headline.poor'), detail: lowering };
    case 'unknown':
      return { title: t('home.headline.unknown'), detail: t('home.headline.unknown.detail') };
    default:
      return assertNever(report.level);
  }
}

const TITLE_KEYS: Readonly<Record<string, MessageKey>> = {
  'disk.critical': 'health.title.disk.critical',
  'disk.low': 'health.title.disk.low',
  'disk.watch': 'health.title.disk.watch',
  'memory.critical': 'health.title.memory.critical',
  'memory.high': 'health.title.memory.high',
  'restart.pending': 'health.title.restart.pending',
  'startup.many': 'health.title.startup.lots',
  'startup.some': 'health.title.startup.some',
};

/**
 * The name of a deduction. The core writes it in English and keeps it as the source; the
 * other languages are looked up by the deduction's stable id.
 */
export function deductionTitle(deduction: DeductionView): string {
  const key = TITLE_KEYS[deduction.id];
  return key && getLocale() !== 'en' ? t(key) : deduction.title;
}

/**
 * The reason of a deduction, written from the code and the figures the core sends with it. In
 * English, and for a code this UI does not know, it is the core's own sentence.
 */
export function deductionReason(deduction: DeductionView): string {
  return say(deduction.reasonPhrase, deduction.reason);
}

function assertNever(value: never): never {
  throw new Error(`Unhandled value: ${JSON.stringify(value)}`);
}
