import { ChevronDown, ChevronRight, ShieldCheck } from 'lucide-react';
import { say, sayCapitalised, t } from '../../shared/i18n';
import type {
  ElevationStatusView,
  TweakInfo,
  TweakPlanView,
  TweakStepView,
} from '../../shared/ipc';
import { elevationReasonText } from '../../shared/text/memory';
import { Badge, Button, ListRow, Switch } from '../../shared/ui';
import { restartNote, tweakDescription, tweakEvidence, tweakTitle } from './messages';

/** What a tweak changes, loaded when the user asks for it. */
export type PlanLoad =
  | { status: 'loading' }
  | { status: 'error'; message: string }
  | { status: 'ready'; plan: TweakPlanView };

/** What a tweak is doing right now, while a command runs. */
export type Busy = 'applying' | 'undoing';

/** The exact changes of a tweak: one line per registry value, service or command. */
export function StepList({ steps, label }: { steps: TweakStepView[]; label: string }) {
  return (
    <ul aria-label={label} className="mt-1 flex flex-col gap-1.5">
      {steps.map((step) => (
        <li key={step.subject} className="rounded-sm bg-surface-sunken px-2 py-1.5">
          <code className="block break-all font-mono text-caption text-fg">
            {say(step.subjectPhrase, step.subject)}
          </code>
          <span className="block text-caption text-muted">
            {say(step.currentPhrase, step.current)} <span aria-hidden>{'->'}</span>
            <span className="sr-only">{t('tweaks.row.changesTo')}</span>{' '}
            {say(step.targetPhrase, step.target)}
          </span>
        </li>
      ))}
    </ul>
  );
}

function evidenceLine(tweak: TweakInfo): string {
  return t('tweaks.row.evidence', {
    level: t(tweak.evidence === 'measured' ? 'tweaks.row.measured' : 'tweaks.row.anecdotal'),
    note: tweakEvidence(tweak),
    restart: restartNote(tweak.restart),
  });
}

/**
 * Whether the switch of an administrator tweak can be used: the helper is available and Volt can
 * undo what is on (a tweak that was on before Volt cannot be switched off by it).
 */
function adminSwitchUsable(tweak: TweakInfo, elevation: ElevationStatusView | null): boolean {
  return (
    elevation?.state === 'available' && (tweak.state.kind !== 'applied' || tweak.appliedByVolt)
  );
}

function stateLine(
  tweak: TweakInfo,
  busy: Busy | undefined,
  elevation: ElevationStatusView | null,
): string | null {
  if (tweak.needsAdmin && busy !== undefined) return t('tweaks.row.waitingAdmin');
  if (busy === 'applying') return t('tweaks.row.turningOn');
  if (busy === 'undoing') return t('tweaks.row.working');
  if (tweak.needsAdmin) {
    if (elevation === null) return t('tweaks.row.needsAdmin');
    if (elevation.state === 'unavailable') return elevationReasonText(elevation.reason);
    if (tweak.state.kind === 'applied' && !tweak.appliedByVolt) return t('tweaks.row.alreadyOn');
    return t('tweaks.row.adminReady');
  }
  const state = tweak.state;
  switch (state.kind) {
    case 'applied':
      return tweak.appliedByVolt ? null : t('tweaks.row.alreadyOn');
    case 'partial':
      return t('tweaks.row.partial');
    case 'unreadable':
      return t('tweaks.row.unreadable', { message: sayCapitalised(state.phrase, state.message) });
    default:
      return null;
  }
}

export interface TweakRowProps {
  tweak: TweakInfo;
  /** Advanced mode: risk badge, evidence note and the list of changes. */
  detailed: boolean;
  busy: Busy | undefined;
  /** Whether administrator actions can be used; `null` until it is known. */
  elevation: ElevationStatusView | null;
  /** The plan, when the user opened "What it changes". Absent when closed. */
  plan: PlanLoad | undefined;
  onToggle: (tweak: TweakInfo, value: boolean) => void;
  onUndo: (tweak: TweakInfo) => void;
  onShowPlan: (tweak: TweakInfo, open: boolean) => void;
}

/** One tweak as a list row, in the simple or the advanced form. */
export function TweakRow({
  tweak,
  detailed,
  busy,
  elevation,
  plan,
  onToggle,
  onUndo,
  onShowPlan,
}: TweakRowProps) {
  const status = stateLine(tweak, busy, elevation);
  const open = plan !== undefined;
  return (
    <ListRow
      title={tweakTitle(tweak)}
      badge={
        detailed || tweak.needsAdmin ? (
          <>
            {detailed ? <Badge risk={tweak.risk} /> : null}
            {tweak.needsAdmin ? (
              <Badge>
                <ShieldCheck aria-hidden size={12} />
                {t('common.administrator')}
              </Badge>
            ) : null}
          </>
        ) : undefined
      }
      description={
        <>
          <span className="block">{tweakDescription(tweak)}</span>
          {detailed ? <span className="mt-0.5 block">{evidenceLine(tweak)}</span> : null}
          {status ? <span className="mt-0.5 block font-semibold text-fg">{status}</span> : null}
          {detailed ? (
            <>
              <button
                type="button"
                aria-expanded={open}
                aria-label={t('tweaks.row.whatChanges.label', { title: tweakTitle(tweak) })}
                className="mt-1 inline-flex items-center gap-1 rounded-sm font-semibold text-accent-text hover:underline"
                onClick={() => onShowPlan(tweak, !open)}
              >
                {open ? (
                  <ChevronDown aria-hidden size={14} />
                ) : (
                  <ChevronRight aria-hidden size={14} />
                )}
                {t('tweaks.row.whatChanges')}
              </button>
              {plan?.status === 'loading' ? (
                <span className="block">{t('tweaks.row.reading')}</span>
              ) : null}
              {plan?.status === 'error' ? <span className="block">{plan.message}</span> : null}
              {plan?.status === 'ready' ? (
                <StepList
                  steps={plan.plan.steps}
                  label={t('tweaks.row.changesOf', { title: tweakTitle(tweak) })}
                />
              ) : null}
            </>
          ) : null}
        </>
      }
      trailing={
        <div className="flex items-center gap-2">
          {tweak.state.kind === 'partial' && !tweak.needsAdmin ? (
            <Button
              size="sm"
              aria-label={t('tweaks.row.undo.label', { title: tweakTitle(tweak) })}
              disabled={busy !== undefined}
              onClick={() => onUndo(tweak)}
            >
              {t('common.undo')}
            </Button>
          ) : null}
          <Switch
            aria-label={tweakTitle(tweak)}
            aria-busy={busy !== undefined}
            checked={tweak.state.kind === 'applied'}
            disabled={
              busy !== undefined ||
              tweak.state.kind === 'unreadable' ||
              (tweak.needsAdmin && !adminSwitchUsable(tweak, elevation))
            }
            onCheckedChange={(value) => onToggle(tweak, value)}
          />
        </div>
      }
    />
  );
}
