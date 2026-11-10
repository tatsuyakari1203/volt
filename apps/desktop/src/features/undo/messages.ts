import {
  type ActivitySource,
  type ActivityStatus,
  describeIpcError,
  type ExportResult,
  IpcCommandError,
  IpcUnavailableError,
  type UndoResult,
} from '../../shared/ipc';

/** A message to show the user after an action, with optional detail lines. */
export interface Notice {
  /** `done` and `info` are calm; `problem` needs attention and is announced as an alert. */
  tone: 'done' | 'info' | 'problem';
  /** Plain-words sentence. */
  message: string;
  /** What failed and why, one line each. */
  details: string[];
}

/** Heading of a row, naming how the recorded event ended. */
export const statusLabels: Record<ActivityStatus, string> = {
  applied: 'Applied',
  reverted: 'Undone',
  failed: 'Failed',
  rolledBack: 'Rolled back',
  other: 'Other',
};

/** Who started a change, in words. */
export const sourceLabels: Record<ActivitySource, string> = {
  manual: 'You',
  automation: 'Automation',
  other: 'Unknown source',
};

/** The time as the user's locale writes it; the raw ISO text when it cannot be parsed. */
export function formatTime(iso: string): string {
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? iso : date.toLocaleString();
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

/** Ends a sentence that may or may not already end with a full stop. */
function sentence(text: string): string {
  return /[.!?]$/.test(text) ? text : `${text}.`;
}

function count(n: number, one: string, many: string): string {
  return `${n} ${n === 1 ? one : many}`;
}

/** Explains how an undo ended. `scope` says whether one change or a whole batch was asked for. */
export function describeUndo(result: UndoResult, scope: 'change' | 'batch'): Notice {
  const details = result.failures.map((failure) => `${failure.what}: ${failure.reason}`);
  switch (result.status) {
    case 'restored':
      return {
        tone: 'done',
        message:
          scope === 'change'
            ? 'Undone. The earlier value is back.'
            : `Undone. ${count(result.restored, 'change is', 'changes are')} back to how ${
                result.restored === 1 ? 'it was' : 'they were'
              }.`,
        details,
      };
    case 'partial':
      return {
        tone: 'problem',
        message: `Only part of it was undone: ${result.restored} restored, ${result.failed} not. What failed is still in place, and you can try again.`,
        details,
      };
    case 'failed':
      return {
        tone: 'problem',
        message:
          scope === 'change'
            ? 'Could not undo this change. It is still in place, and you can try again.'
            : 'Nothing could be undone. The changes are still in place, and you can try again.',
        details,
      };
    case 'nothingToUndo':
      return {
        tone: 'info',
        message: 'Nothing to undo here. It was already undone, or Volt did not change it.',
        details,
      };
    default:
      return assertNever(result.status);
  }
}

/** Explains an undo command that failed outright (nothing came back). */
export function describeUndoFailure(error: unknown): Notice {
  return {
    tone: 'problem',
    message: `Could not undo. ${sentence(describeError(error))}`,
    details: [],
  };
}

/** Explains how saving the export ended; `null` when the user closed the dialog (nothing to say). */
export function describeSaved(result: ExportResult): Notice | null {
  switch (result.kind) {
    case 'saved':
      return {
        tone: 'done',
        message: `Saved ${count(result.records, 'record', 'records')} to ${result.path}.`,
        details: [],
      };
    case 'cancelled':
      return null;
    default:
      return assertNever(result);
  }
}

/** Explains a save that failed, and points at the copy fallback. */
export function describeSaveFailure(error: unknown): Notice {
  return {
    tone: 'problem',
    message: `Could not save the file. ${sentence(describeError(error))} You can copy the text instead.`,
    details: [],
  };
}

/** Explains a clear that failed. */
export function describeClearFailure(error: unknown): Notice {
  return {
    tone: 'problem',
    message: `Could not clear the history. ${sentence(describeError(error))}`,
    details: [],
  };
}

/** Confirms that the history was cleared. */
export function describeCleared(removed: number): Notice {
  return {
    tone: 'done',
    message:
      removed === 0
        ? 'Nothing to clear: every record belongs to a change that can still be undone.'
        : `Cleared ${count(removed, 'record', 'records')}. Changes that can be undone stay in the list.`,
    details: [],
  };
}

/** Confirms a copy to the clipboard. */
export function describeCopied(records: number): Notice {
  return {
    tone: 'done',
    message: `Copied ${count(records, 'record', 'records')} to the clipboard.`,
    details: [],
  };
}

/** Explains a copy that failed. */
export function describeCopyFailure(error: unknown): Notice {
  return {
    tone: 'problem',
    message: `Could not copy the text. ${sentence(describeError(error))} You can save it as a file instead.`,
    details: [],
  };
}

function assertNever(value: never): never {
  throw new Error(`Unhandled value: ${JSON.stringify(value)}`);
}
