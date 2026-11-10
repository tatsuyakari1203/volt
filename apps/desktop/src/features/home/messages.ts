import {
  describeIpcError,
  type FixActionView,
  type HealthCheckView,
  type HealthLevelView,
  type HealthView,
  IpcCommandError,
  IpcUnavailableError,
} from '../../shared/ipc';

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
      return { target: 'clean', label: 'Open Cleaner' };
    case 'openMemory':
      return { target: 'memory', label: 'Open Memory' };
    case 'openStartup':
      return { target: 'startup', label: 'Open Startup' };
    case 'restartWindows':
      return null;
    default:
      return assertNever(fix);
  }
}

/** A sentence that stands in for the button of a fix that has none. */
export function fixHint(fix: FixActionView | null): string | null {
  return fix === 'restartWindows' ? 'Restart Windows when you are ready to clear it.' : null;
}

/** The name of a check, for the line that says which checks could not run. */
export function checkName(check: HealthCheckView): string {
  switch (check) {
    case 'systemDisk':
      return 'free disk space';
    case 'memory':
      return 'memory use';
    case 'pendingRestart':
      return 'pending restarts';
    case 'startup':
      return 'startup apps';
    default:
      return assertNever(check);
  }
}

/** The word under the score. */
export function levelLabel(level: HealthLevelView): string {
  switch (level) {
    case 'good':
      return 'Good';
    case 'fair':
      return 'Fair';
    case 'poor':
      return 'Poor';
    case 'unknown':
      return 'Not checked';
    default:
      return assertNever(level);
  }
}

/** The headline and the sentence under it, from the grade and the number of deductions. */
export function headline(report: HealthView): { title: string; detail: string } {
  const count = report.deductions.length;
  const lowering =
    count === 0
      ? 'Nothing is lowering the score.'
      : `${count} ${count === 1 ? 'thing lowers' : 'things lower'} the score. Each one is listed below.`;
  switch (report.level) {
    case 'good':
      return { title: 'Your PC is in good shape', detail: lowering };
    case 'fair':
      return { title: 'Your PC could be better', detail: lowering };
    case 'poor':
      return { title: 'Your PC needs attention', detail: lowering };
    case 'unknown':
      return {
        title: 'Your PC could not be checked',
        detail: 'Volt could not read what it needs to give a score.',
      };
    default:
      return assertNever(report.level);
  }
}

/** One sentence for any error that reached the UI. */
export function describeError(error: unknown): string {
  if (error instanceof IpcUnavailableError) {
    return 'This is only available inside the Volt app.';
  }
  if (error instanceof IpcCommandError) {
    return describeIpcError(error.error);
  }
  return error instanceof Error ? error.message : String(error);
}

function assertNever(value: never): never {
  throw new Error(`Unhandled value: ${JSON.stringify(value)}`);
}
