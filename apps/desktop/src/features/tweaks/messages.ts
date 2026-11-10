import {
  type ApplyResult,
  describeIpcError,
  IpcCommandError,
  IpcUnavailableError,
  type RestartKind,
  type RevertResult,
  type TweakInfo,
} from '../../shared/ipc';

/** What to tell the user about a finished action. */
export interface Outcome {
  /** Plain-words sentence. */
  message: string;
  /** `done` is good news (a toast); `problem` needs the user's attention (a persistent notice). */
  tone: 'done' | 'problem';
  /** Whether the action left something the Undo button of the toast can take back. */
  undoable: boolean;
}

/** What the user still has to do before a tweak takes full effect. */
export function restartNote(restart: RestartKind): string {
  switch (restart) {
    case 'none':
      return '';
    case 'signOut':
      return ' Sign out and back in to finish.';
    case 'reboot':
      return ' Restart your PC to finish.';
    default:
      return assertNever(restart);
  }
}

function assertNever(value: never): never {
  throw new Error(`Unhandled value: ${JSON.stringify(value)}`);
}

function reason(detail: string | null): string {
  return detail ? ` Reason: ${detail}.` : '';
}

/** Explains how an apply ended, in words a person can act on. */
export function describeApply(tweak: TweakInfo, result: ApplyResult): Outcome {
  switch (result.status) {
    case 'applied':
      return {
        message: `"${tweak.title}" is on.${restartNote(tweak.restart)}`,
        tone: 'done',
        undoable: true,
      };
    case 'alreadyApplied':
      return {
        message: `"${tweak.title}" was already on. Nothing was changed.`,
        tone: 'done',
        undoable: false,
      };
    case 'rolledBack':
      return {
        message: `Could not turn on "${tweak.title}". Volt put everything back as it was.${reason(result.detail)}`,
        tone: 'problem',
        undoable: false,
      };
    case 'needsAttention':
      return {
        message: `"${tweak.title}" may be only partly on, and Volt could not confirm that everything was put back.${reason(result.detail)} Use Undo on this tweak to restore your earlier setting.`,
        tone: 'problem',
        undoable: false,
      };
    default:
      return assertNever(result.status);
  }
}

/** Explains how a revert ended. `afterUndo` is true when it answers the Undo button of a toast. */
export function describeRevert(
  tweak: TweakInfo,
  result: RevertResult,
  afterUndo: boolean,
): Outcome {
  switch (result.status) {
    case 'reverted':
      return {
        message: afterUndo
          ? `Undone. "${tweak.title}" is back to how it was.`
          : `"${tweak.title}" is off again.`,
        tone: 'done',
        undoable: false,
      };
    case 'incomplete':
      return {
        message: `Only part of "${tweak.title}" could be undone (${result.restored} restored, ${result.failed} not).${reason(result.detail)} You can try again.`,
        tone: 'problem',
        undoable: false,
      };
    case 'nothingToRevert':
      return {
        message: `Volt has nothing to undo for "${tweak.title}". Volt did not turn it on, so it has no saved earlier setting to restore. You can change it in Windows yourself.`,
        tone: 'problem',
        undoable: false,
      };
    default:
      return assertNever(result.status);
  }
}

/** Explains a command that failed outright (nothing came back), for example a store error. */
export function describeFailure(
  action: 'turn on' | 'undo',
  tweak: TweakInfo,
  error: unknown,
): string {
  const what = action === 'turn on' ? `turn on "${tweak.title}"` : `undo "${tweak.title}"`;
  return `Could not ${what}. ${describeError(error)}`;
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
