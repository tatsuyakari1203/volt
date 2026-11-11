import { type MessageKey, say, t, tx } from '../../shared/i18n';
import type {
  ApplyResult,
  PhraseView,
  RestartKind,
  RevertResult,
  TweakCategory,
  TweakInfo,
} from '../../shared/ipc';
import { describeError } from '../../shared/text/errors';

export { describeError };

/** What to tell the user about a finished action. */
export interface Outcome {
  /** Plain-words sentence. */
  message: string;
  /** `done` is good news (a toast); `problem` needs the user's attention (a persistent notice). */
  tone: 'done' | 'problem';
  /** Whether the action left something the Undo button of the toast can take back. */
  undoable: boolean;
}

/** The name of a tweak. The core writes it in English; other languages are filed under its id. */
export function tweakTitle(tweak: TweakInfo): string {
  return tx(`tweak.${tweak.id}.title`, tweak.title);
}

/** What a tweak does and why, in the current language. */
export function tweakDescription(tweak: TweakInfo): string {
  return tx(`tweak.${tweak.id}.description`, tweak.description);
}

/** Where the claim about a tweak's benefit comes from, in the current language. */
export function tweakEvidence(tweak: TweakInfo): string {
  return tx(`tweak.${tweak.id}.evidence`, tweak.evidenceNote);
}

/** What the user still has to do before a tweak takes full effect, with a space in front. */
export function restartNote(restart: RestartKind): string {
  switch (restart) {
    case 'none':
      return '';
    case 'signOut':
      return ` ${t('tweaks.restart.signOut')}`;
    case 'reboot':
      return ` ${t('tweaks.restart.reboot')}`;
    default:
      return assertNever(restart);
  }
}

function assertNever(value: never): never {
  throw new Error(`Unhandled value: ${JSON.stringify(value)}`);
}

/** The reason a step failed, as the end of a sentence, with a space in front. */
function reason(detail: string | null, phrase: PhraseView | null): string {
  return detail ? ` ${t('tweaks.reason', { detail: say(phrase, detail) })}` : '';
}

/** Explains how an apply ended, in words a person can act on. */
export function describeApply(tweak: TweakInfo, result: ApplyResult): Outcome {
  const title = tweakTitle(tweak);
  switch (result.status) {
    case 'applied':
      return {
        message: t('tweaks.apply.applied', { title, restart: restartNote(tweak.restart) }),
        tone: 'done',
        undoable: true,
      };
    case 'alreadyApplied':
      return { message: t('tweaks.apply.alreadyOn', { title }), tone: 'done', undoable: false };
    case 'rolledBack':
      return {
        message: t('tweaks.apply.rolledBack', {
          title,
          reason: reason(result.detail, result.detailPhrase),
        }),
        tone: 'problem',
        undoable: false,
      };
    case 'needsAttention':
      return {
        message: t('tweaks.apply.needsAttention', {
          title,
          reason: reason(result.detail, result.detailPhrase),
        }),
        tone: 'problem',
        undoable: false,
      };
    case 'declined':
      return { message: t('tweaks.apply.declined', { title }), tone: 'problem', undoable: false };
    case 'notConfirmed':
      return {
        message: t('tweaks.apply.notConfirmed', { title }),
        tone: 'problem',
        undoable: false,
      };
    case 'blocked':
      return { message: t('tweaks.apply.blocked', { title }), tone: 'problem', undoable: false };
    case 'restorePointFailed':
      return {
        message: t('tweaks.apply.restorePoint', { title }),
        tone: 'problem',
        undoable: false,
      };
    case 'timedOut':
      return { message: t('tweaks.apply.timedOut', { title }), tone: 'problem', undoable: false };
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
  const title = tweakTitle(tweak);
  switch (result.status) {
    case 'reverted':
      return {
        message: t(afterUndo ? 'tweaks.revert.undone' : 'tweaks.revert.off', { title }),
        tone: 'done',
        undoable: false,
      };
    case 'incomplete':
      return {
        message: t('tweaks.revert.incomplete', {
          title,
          restored: result.restored,
          failed: result.failed,
          reason: reason(result.detail, result.detailPhrase),
        }),
        tone: 'problem',
        undoable: false,
      };
    case 'nothingToRevert':
      return { message: t('tweaks.revert.nothing', { title }), tone: 'problem', undoable: false };
    case 'declined':
      return { message: t('tweaks.revert.declined', { title }), tone: 'problem', undoable: false };
    case 'notConfirmed':
      return {
        message: t('tweaks.revert.notConfirmed', { title }),
        tone: 'problem',
        undoable: false,
      };
    case 'blocked':
      return { message: t('tweaks.revert.blocked', { title }), tone: 'problem', undoable: false };
    case 'timedOut':
      return { message: t('tweaks.revert.timedOut', { title }), tone: 'problem', undoable: false };
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
  return t(action === 'turn on' ? 'tweaks.fail.turnOn' : 'tweaks.fail.undo', {
    title: tweakTitle(tweak),
    error: describeError(error),
  });
}

/** Every category, in the order the screen lists them. */
export const CATEGORY_ORDER: readonly TweakCategory[] = [
  'appearance',
  'performance',
  'gaming',
  'privacy',
  'network',
  'windowsUpdate',
  'security',
  'debloat',
];

const CATEGORY_KEYS: Record<TweakCategory, MessageKey> = {
  appearance: 'tweaks.category.appearance',
  performance: 'tweaks.category.performance',
  gaming: 'tweaks.category.gaming',
  privacy: 'tweaks.category.privacy',
  network: 'tweaks.category.network',
  windowsUpdate: 'tweaks.category.windowsUpdate',
  security: 'tweaks.category.security',
  debloat: 'tweaks.category.debloat',
};

/** Display name of a category. */
export function categoryLabel(category: TweakCategory): string {
  return t(CATEGORY_KEYS[category]);
}

/** Explains how turning off a tweak that Volt did not apply ended. */
export function describeReset(tweak: TweakInfo, result: RevertResult): Outcome {
  const title = tweakTitle(tweak);
  if (result.status === 'reverted') {
    return { message: t('tweaks.reset.off', { title }), tone: 'done', undoable: false };
  }
  return {
    message: t('tweaks.reset.failed', {
      title,
      reason: reason(result.detail, result.detailPhrase),
    }),
    tone: 'problem',
    undoable: false,
  };
}
