import { ChevronDown, ChevronRight } from 'lucide-react';
import { t } from '../../shared/i18n';
import type { TweakInfo, TweakPlanView, TweakStepView } from '../../shared/ipc';
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
          <code className="block break-all font-mono text-caption text-fg">{step.subject}</code>
          <span className="block text-caption text-muted">
            {step.current} <span aria-hidden>{'->'}</span>
            <span className="sr-only">{t('tweaks.row.changesTo')}</span> {step.target}
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

function stateLine(tweak: TweakInfo, busy: Busy | undefined): string | null {
  if (busy === 'applying') return t('tweaks.row.turningOn');
  if (busy === 'undoing') return t('tweaks.row.working');
  if (tweak.needsAdmin) return t('tweaks.row.needsAdmin');
  const state = tweak.state;
  switch (state.kind) {
    case 'applied':
      return tweak.appliedByVolt ? null : t('tweaks.row.alreadyOn');
    case 'partial':
      return t('tweaks.row.partial');
    case 'unreadable':
      return t('tweaks.row.unreadable', { message: state.message });
    default:
      return null;
  }
}

export interface TweakRowProps {
  tweak: TweakInfo;
  /** Advanced mode: risk badge, evidence note and the list of changes. */
  detailed: boolean;
  busy: Busy | undefined;
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
  plan,
  onToggle,
  onUndo,
  onShowPlan,
}: TweakRowProps) {
  const status = stateLine(tweak, busy);
  const open = plan !== undefined;
  return (
    <ListRow
      title={tweakTitle(tweak)}
      badge={detailed ? <Badge risk={tweak.risk} /> : undefined}
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
            disabled={busy !== undefined || tweak.needsAdmin || tweak.state.kind === 'unreadable'}
            onCheckedChange={(value) => onToggle(tweak, value)}
          />
        </div>
      }
    />
  );
}
