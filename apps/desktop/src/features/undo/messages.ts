import { getLocale, localeTag, say, sayCapitalised, t, tn, tx } from '../../shared/i18n';
import type {
  ActivityItem,
  ActivitySource,
  ActivityStatus,
  ExportResult,
  UndoResult,
} from '../../shared/ipc';
import { describeError } from '../../shared/text/errors';

export { describeError };

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
export function statusLabel(status: ActivityStatus): string {
  switch (status) {
    case 'applied':
      return t('undo.status.applied');
    case 'reverted':
      return t('undo.status.reverted');
    case 'failed':
      return t('undo.status.failed');
    case 'rolledBack':
      return t('undo.status.rolledBack');
    case 'other':
      return t('undo.status.other');
    default:
      return assertNever(status);
  }
}

/** Who started a change, in words. */
export function sourceLabel(source: ActivitySource): string {
  switch (source) {
    case 'manual':
      return t('undo.source.manual');
    case 'automation':
      return t('undo.source.automation');
    case 'other':
      return t('undo.source.other');
    default:
      return assertNever(source);
  }
}

/**
 * What a record is about, named by its label. The core writes the name in English: a tweak by its
 * title, the rest by a fixed name. Another language is looked up by the label, which is stable.
 */
export function activityTitle(item: ActivityItem): string | null {
  const { label, title } = item;
  if (title === null || label === null || getLocale() === 'en') return title;
  switch (label) {
    case 'startup':
      return t('undo.title.startup');
    case 'memory.clean':
      return t('undo.title.memory');
    case 'process.end':
      return t('undo.title.endedApps');
    case 'process.tune':
      return t('undo.title.processSettings');
    case 'recycle-bin':
      return t('clean.bin.title');
    default:
      break;
  }
  if (label.endsWith(':reset')) {
    const id = label.slice(0, -':reset'.length);
    const english = title.replace(/ \(set to the Windows default\)$/, '');
    return t('undo.title.reset', { title: tx(`tweak.${id}.title`, english) });
  }
  const tweak = tx(`tweak.${label}.title`, '');
  if (tweak) return tweak;
  const rule = tx(`rule.${label}.title`, '');
  if (rule) return t('undo.title.cleaning', { title: rule });
  return title;
}

/** The time as the user's locale writes it; the raw ISO text when it cannot be parsed. */
export function formatTime(iso: string): string {
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? iso : date.toLocaleString(localeTag());
}

/** Ends a sentence that may or may not already end with a full stop. */
function sentence(text: string): string {
  return /[.!?]$/.test(text) ? text : `${text}.`;
}

/** Explains how an undo ended. `scope` says whether one change or a whole batch was asked for. */
export function describeUndo(result: UndoResult, scope: 'change' | 'batch'): Notice {
  const details = result.failures.map(
    (failure) =>
      `${sayCapitalised(failure.whatPhrase, failure.what)}: ${say(failure.reasonPhrase, failure.reason)}`,
  );
  switch (result.status) {
    case 'restored':
      return {
        tone: 'done',
        message:
          scope === 'change' ? t('undo.done.change') : tn('undo.done.batch', result.restored),
        details,
      };
    case 'partial':
      return {
        tone: 'problem',
        message: t('undo.partial', { restored: result.restored, failed: result.failed }),
        details,
      };
    case 'failed':
      return {
        tone: 'problem',
        message: t(scope === 'change' ? 'undo.failed.change' : 'undo.failed.batch'),
        details,
      };
    case 'nothingToUndo':
      return { tone: 'info', message: t('undo.nothing'), details };
    case 'needsAcknowledgement':
      return {
        tone: 'problem',
        message: t('undo.needsAcknowledgement'),
        details: result.overwrites.map((found) =>
          t('undo.changedLater', { what: sayCapitalised(found.whatPhrase, found.what) }),
        ),
      };
    default:
      return assertNever(result.status);
  }
}

/** Explains an undo command that failed outright (nothing came back). */
export function describeUndoFailure(error: unknown): Notice {
  return {
    tone: 'problem',
    message: t('undo.failed.command', { error: sentence(describeError(error)) }),
    details: [],
  };
}

/** Explains how saving the export ended; `null` when the user closed the dialog (nothing to say). */
export function describeSaved(result: ExportResult): Notice | null {
  switch (result.kind) {
    case 'saved':
      return {
        tone: 'done',
        message: t('undo.saved', {
          records: tn('count.record', result.records),
          path: result.path,
        }),
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
    message: t('undo.saveFailed', { error: sentence(describeError(error)) }),
    details: [],
  };
}

/** Explains a clear that failed. */
export function describeClearFailure(error: unknown): Notice {
  return {
    tone: 'problem',
    message: t('undo.clearFailed', { error: sentence(describeError(error)) }),
    details: [],
  };
}

/** Confirms that the history was cleared. */
export function describeCleared(removed: number, kept: number): Notice {
  const stays = kept === 0 ? '' : ` ${tn('undo.cleared.kept', kept)}`;
  return {
    tone: 'done',
    message:
      (removed === 0
        ? t('undo.cleared.none')
        : t('undo.cleared.some', { records: tn('count.record', removed) })) + stays,
    details: [],
  };
}

/** Confirms a copy to the clipboard. */
export function describeCopied(records: number): Notice {
  return {
    tone: 'done',
    message: t('undo.copied', { records: tn('count.record', records) }),
    details: [],
  };
}

/** Explains a copy that failed. */
export function describeCopyFailure(error: unknown): Notice {
  return {
    tone: 'problem',
    message: t('undo.copyFailed', { error: sentence(describeError(error)) }),
    details: [],
  };
}

function assertNever(value: never): never {
  throw new Error(`Unhandled value: ${JSON.stringify(value)}`);
}
