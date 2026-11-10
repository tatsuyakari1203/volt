import {
  describeIpcError,
  IpcCommandError,
  IpcUnavailableError,
  type StartupChangeResult,
  type StartupEntryView,
} from '../../shared/ipc';

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

/** Explains how a change ended, in words a person can act on. */
export function describeChange(
  entry: StartupEntryView,
  wanted: boolean,
  result: StartupChangeResult,
): Outcome {
  const name = `"${entry.name}"`;
  switch (result.status) {
    case 'alreadyInState':
      return {
        message: `${name} was already ${wanted ? 'on' : 'off'}. Nothing was changed.`,
        tone: 'done',
      };
    case 'changed':
      if (result.enabled === null) {
        return {
          message: `${name} was changed, but Volt could not read it back to confirm. Reload the list to check.`,
          tone: 'problem',
        };
      }
      return {
        message: result.enabled
          ? `${name} will start with Windows again. You can undo this on the Undo page.`
          : `${name} will no longer start with Windows. It stays in this list, and you can undo this on the Undo page.`,
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
  const action = wanted ? 'turn on' : 'turn off';
  if (error instanceof IpcCommandError) {
    switch (error.error.kind) {
      case 'accessDenied':
        return entry.needsAdmin
          ? `Changing "${entry.name}" needs administrator rights. Volt cannot do that yet.`
          : `Windows refused to ${action} "${entry.name}" (access denied). Nothing was changed.`;
      case 'notFound':
        return `"${entry.name}" is no longer in Windows' startup list, so nothing was changed. The list was reloaded.`;
      default:
        break;
    }
  }
  return `Could not ${action} "${entry.name}". ${describeError(error)}`;
}

/** How a source could not be read, for the notice above the list. */
export function describeProblems(
  problems: ReadonlyArray<{ sourceLabel: string; reason: string }>,
): string | null {
  if (problems.length === 0) return null;
  const parts = problems.map((problem) => `${problem.sourceLabel} (${problem.reason})`);
  return `Some places could not be read, so their entries are missing: ${parts.join(', ')}.`;
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
