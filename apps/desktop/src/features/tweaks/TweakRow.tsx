import { ChevronDown, ChevronRight } from 'lucide-react';
import type { TweakInfo, TweakPlanView, TweakStepView } from '../../shared/ipc';
import { Badge, Button, ListRow, Switch } from '../../shared/ui';
import { restartNote } from './messages';

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
            <span className="sr-only">changes to</span> {step.target}
          </span>
        </li>
      ))}
    </ul>
  );
}

function evidenceLine(tweak: TweakInfo): string {
  const label = tweak.evidence === 'measured' ? 'Measured' : 'Anecdotal';
  return `Evidence: ${label}. ${tweak.evidenceNote}${restartNote(tweak.restart)}`;
}

function stateLine(tweak: TweakInfo, busy: Busy | undefined): string | null {
  if (busy === 'applying') return 'Turning on...';
  if (busy === 'undoing') return 'Working...';
  if (tweak.needsAdmin) return 'Needs administrator rights. It cannot be changed from here yet.';
  const state = tweak.state;
  switch (state.kind) {
    case 'applied':
      return tweak.appliedByVolt
        ? null
        : 'Already on, but Volt did not turn it on, so it has no earlier value to restore.';
    case 'partial':
      return 'Only partly on. Turn it on to finish, or use Undo to put things back.';
    case 'unreadable':
      return `Could not read the current setting. ${state.message}`;
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
      title={tweak.title}
      badge={detailed ? <Badge risk={tweak.risk} /> : undefined}
      description={
        <>
          <span className="block">{tweak.description}</span>
          {detailed ? <span className="mt-0.5 block">{evidenceLine(tweak)}</span> : null}
          {status ? <span className="mt-0.5 block font-semibold text-fg">{status}</span> : null}
          {detailed ? (
            <>
              <button
                type="button"
                aria-expanded={open}
                aria-label={`What ${tweak.title} changes`}
                className="mt-1 inline-flex items-center gap-1 rounded-sm font-semibold text-accent-text hover:underline"
                onClick={() => onShowPlan(tweak, !open)}
              >
                {open ? (
                  <ChevronDown aria-hidden size={14} />
                ) : (
                  <ChevronRight aria-hidden size={14} />
                )}
                What it changes
              </button>
              {plan?.status === 'loading' ? <span className="block">Reading...</span> : null}
              {plan?.status === 'error' ? <span className="block">{plan.message}</span> : null}
              {plan?.status === 'ready' ? (
                <StepList steps={plan.plan.steps} label={`Changes of ${tweak.title}`} />
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
              aria-label={`Undo ${tweak.title}`}
              disabled={busy !== undefined}
              onClick={() => onUndo(tweak)}
            >
              Undo
            </Button>
          ) : null}
          <Switch
            aria-label={tweak.title}
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
