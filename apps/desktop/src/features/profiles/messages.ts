import { say, t, tx } from '../../shared/i18n';
import type {
  NotIncludedView,
  ProfileView,
  StepStatusView,
  SwitchStepView,
} from '../../shared/ipc';

export { describeError } from '../../shared/text/errors';

/** A tweak's name in the current language. */
export function tweakName(id: string, english: string): string {
  return tx(`tweak.${id}.title`, english);
}

/** Why a tweak of a profile is not changed by a switch, in words. */
export function leftOutText(item: NotIncludedView): string {
  switch (item.reason) {
    case 'needsAdministrator':
      return t('profiles.leftOut.admin');
    case 'unknown':
      return t('profiles.leftOut.unknown');
    case 'notAppliedByVolt':
      return t('profiles.leftOut.notByVolt');
    default:
      return t('profiles.leftOut.unavailable', { detail: item.detail ?? '' }).trim();
  }
}

/** What a profile sets in automation, one short phrase, or `null` when it sets nothing. */
export function automationSummary(profile: ProfileView): string | null {
  const parts: string[] = [];
  const { memory, cleaning } = profile.automation;
  if (memory) {
    parts.push(
      t(memory.enabled ? 'profiles.automation.memoryOn' : 'profiles.automation.memoryOff'),
    );
  }
  if (cleaning) {
    parts.push(
      t(cleaning.enabled ? 'profiles.automation.cleaningOn' : 'profiles.automation.cleaningOff'),
    );
  }
  return parts.length > 0 ? parts.join('; ') : null;
}

const STEP_KEYS = {
  nothing: 'profiles.step.nothing',
  rolledBack: 'profiles.step.rolledBack',
  needsAttention: 'profiles.step.needsAttention',
  failed: 'profiles.step.failed',
  notAttempted: 'profiles.step.notAttempted',
} as const satisfies Record<Exclude<StepStatusView, 'done'>, string>;

/** How one tweak of a switch ended, in words. */
export function stepText(step: SwitchStepView): string {
  if (step.status === 'done') {
    return t(step.turnedOn ? 'profiles.step.turnedOn' : 'profiles.step.turnedOff');
  }
  const base = t(STEP_KEYS[step.status]);
  return step.detail === null ? base : `${base} ${say(step.detailPhrase, step.detail)}`;
}
