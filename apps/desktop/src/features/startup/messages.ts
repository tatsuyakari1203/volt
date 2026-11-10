import { t } from '../../shared/i18n';
import {
  IpcCommandError,
  type StartupChangeResult,
  type StartupEntryView,
  type StartupSource,
} from '../../shared/ipc';
import { describeError } from '../../shared/text/errors';

export { describeError };

/** What to tell the user about a finished change. */
export interface Outcome {
  /** Plain-words sentence. */
  message: string;
  /** `done` is good news (a toast); `problem` needs the user's attention (a persistent notice). */
  tone: 'done' | 'problem';
}

function assertNever(value: never): never {
  throw new Error(`Unhandled value: ${JSON.stringify(value)}`);
}

/** Where an entry comes from, in the current language. The core's own words for an unknown one. */
export function sourceName(source: StartupSource, english: string): string {
  switch (source) {
    case 'userRun':
      return t('startup.source.userRun');
    case 'machineRun':
      return t('startup.source.machineRun');
    case 'machineRun32':
      return t('startup.source.machineRun32');
    case 'userFolder':
      return t('startup.source.userFolder');
    case 'commonFolder':
      return t('startup.source.commonFolder');
    default:
      // `other` is a source this UI does not know: the core's own words are the best there are.
      return english;
  }
}

/** Explains how a change ended, in words a person can act on. */
export function describeChange(
  entry: StartupEntryView,
  wanted: boolean,
  result: StartupChangeResult,
): Outcome {
  const name = entry.name;
  switch (result.status) {
    case 'alreadyInState':
      return {
        message: t(wanted ? 'startup.change.alreadyOn' : 'startup.change.alreadyOff', { name }),
        tone: 'done',
      };
    case 'changed':
      if (result.enabled === null) {
        return { message: t('startup.change.unconfirmed', { name }), tone: 'problem' };
      }
      return {
        message: t(result.enabled ? 'startup.change.on' : 'startup.change.off', { name }),
        tone: 'done',
      };
    default:
      return assertNever(result.status);
  }
}

/** Explains a change that failed outright. */
export function describeChangeFailure(
  entry: StartupEntryView,
  wanted: boolean,
  error: unknown,
): string {
  const name = entry.name;
  if (error instanceof IpcCommandError) {
    switch (error.error.kind) {
      case 'accessDenied':
        return entry.needsAdmin
          ? t('startup.fail.needsAdmin', { name })
          : t(wanted ? 'startup.fail.deniedOn' : 'startup.fail.deniedOff', { name });
      case 'notFound':
        return t('startup.fail.notFound', { name });
      default:
        break;
    }
  }
  return t(wanted ? 'startup.fail.turnOn' : 'startup.fail.turnOff', {
    name,
    error: describeError(error),
  });
}

/** The class of a failure the core names in English, in the current language when it is known. */
function problemReason(reason: string): string {
  switch (reason) {
    case 'access denied':
      return t('startup.problem.accessDenied');
    case 'not found':
      return t('startup.problem.notFound');
    default:
      return reason;
  }
}

/** How a source could not be read, for the notice above the list. */
export function describeProblems(
  problems: ReadonlyArray<{ source: StartupSource; sourceLabel: string; reason: string }>,
): string | null {
  if (problems.length === 0) return null;
  const parts = problems.map(
    (problem) =>
      `${sourceName(problem.source, problem.sourceLabel)} (${problemReason(problem.reason)})`,
  );
  return t('startup.problems', { parts: parts.join(', ') });
}

/**
 * Shortens a command line for a list row: the start and the end are kept, because the program
 * is at the start and a file name or option at the end. A command that already fits is returned
 * as it is. The full text stays available in the row.
 */
export function shortenCommand(command: string, max = 56): string {
  const text = command.trim();
  if (text.length <= max) return text;
  const tail = Math.floor((max - 1) / 3);
  const head = max - 1 - tail;
  return `${text.slice(0, head).trimEnd()}…${text.slice(text.length - tail).trimStart()}`;
}

/** One sentence when the scheduled tasks could not be read. */
export function describeTasksProblem(reason: string | null): string | null {
  return reason ? t('startup.tasks.problem', { reason }) : null;
}
