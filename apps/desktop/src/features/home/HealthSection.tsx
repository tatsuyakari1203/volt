import { Loader2, RefreshCw, TriangleAlert } from 'lucide-react';
import type { HealthView } from '../../shared/ipc';
import { Badge, Button, Card, ListGroup, ListRow, ProgressRing } from '../../shared/ui';
import type { HealthState } from './hooks';
import { checkName, fixHint, fixLink, type HomeTarget, headline, levelLabel } from './messages';

/** The time of a check as a short local time. A malformed time is left out, not guessed. */
function checkedAtText(iso: string): string | null {
  const time = new Date(iso);
  if (Number.isNaN(time.getTime())) return null;
  return time.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
}

function ScoreRing({ report }: { report: HealthView }) {
  if (report.level === 'unknown') {
    // No check ran, so there is no number to put in a progress ring.
    return (
      <div
        role="img"
        aria-label="Health score not available"
        className="grid size-[136px] shrink-0 place-items-center rounded-full border-10 border-line text-center"
      >
        <div>
          <div className="text-display font-semibold">—</div>
          <div className="text-body text-muted">{levelLabel(report.level)}</div>
        </div>
      </div>
    );
  }
  return (
    <ProgressRing value={report.score} label="Health score" size={136} strokeWidth={10}>
      <div>
        <div className="text-display font-semibold tabular-nums">{report.score}</div>
        <div className="text-body text-muted">{levelLabel(report.level)}</div>
      </div>
    </ProgressRing>
  );
}

/** The score, why it is what it is, and when it was worked out. */
function ScoreCard({ report, state }: { report: HealthView; state: HealthState }) {
  const { title, detail } = headline(report);
  const checkedAt = checkedAtText(report.checkedAt);
  return (
    <Card
      as="section"
      aria-label="Health score"
      className="flex flex-col items-center gap-4 py-5 wide:flex-row wide:gap-8 wide:px-8 wide:py-6"
    >
      <ScoreRing report={report} />
      <div className="flex w-full flex-col items-center gap-3 text-center wide:items-start wide:text-left">
        <div>
          <h2 className="text-subtitle font-semibold">{title}</h2>
          <p className="mt-1 text-body text-muted">{detail}</p>
        </div>
        <div className="flex flex-col items-center gap-2 wide:flex-row wide:gap-3">
          <Button
            size="md"
            loading={state.checking}
            icon={<RefreshCw aria-hidden size={16} />}
            onClick={state.check}
          >
            {state.checking ? 'Checking...' : 'Check again'}
          </Button>
          {checkedAt ? <p className="text-caption text-muted">Checked at {checkedAt}</p> : null}
        </div>
      </div>
    </Card>
  );
}

function Notice({ children }: { children: string }) {
  return (
    <div
      role="status"
      className="flex items-start gap-3 rounded-md bg-caution p-3 text-body text-caution-fg"
    >
      <TriangleAlert aria-hidden size={18} className="mt-0.5 shrink-0" />
      <p className="min-w-0 flex-1">{children}</p>
    </div>
  );
}

/** Each deduction with its points, its reason and the button that leads to the fix. */
export function Deductions({
  report,
  onNavigate,
}: {
  report: HealthView | null;
  onNavigate: (target: HomeTarget) => void;
}) {
  if (report === null || report.deductions.length === 0) return null;
  return (
    <section aria-labelledby="home-deductions">
      <h2 id="home-deductions" className="text-subtitle font-semibold">
        What lowers the score
      </h2>
      <p className="pt-0.5 pb-2 text-caption text-muted">
        The score is 100 minus the points below, and nothing else.
      </p>
      <ListGroup aria-label="Deductions">
        {report.deductions.map((deduction) => {
          const link = fixLink(deduction.fix);
          const hint = fixHint(deduction.fix);
          return (
            <ListRow
              key={deduction.id}
              title={deduction.title}
              badge={<Badge>{`−${deduction.points} points`}</Badge>}
              description={
                <>
                  <span className="block">{deduction.reason}</span>
                  {hint ? <span className="mt-0.5 block font-semibold text-fg">{hint}</span> : null}
                </>
              }
              trailing={
                link ? (
                  <Button
                    size="sm"
                    aria-label={`${link.label}: ${deduction.title}`}
                    onClick={() => onNavigate(link.target)}
                  >
                    {link.label}
                  </Button>
                ) : null
              }
            />
          );
        })}
      </ListGroup>
    </section>
  );
}

/**
 * The health score: loading, error, the score card and the checks that could not run. The
 * deductions behind the score are [`Deductions`], which the screen places below the live cards.
 * Rendered only from what the shell reports.
 */
export function HealthScore({ state }: { state: HealthState }) {
  const { report, checking, error, check } = state;

  if (report === null && checking) {
    return (
      <Card
        role="status"
        aria-label="Health score"
        className="flex items-center justify-center gap-2 py-10 text-muted"
      >
        <Loader2 aria-hidden size={18} className="animate-spin motion-reduce:animate-none" />
        <p>Checking your PC...</p>
      </Card>
    );
  }

  if (report === null) {
    return (
      <Card
        role="alert"
        aria-label="Health score"
        className="flex flex-col items-center gap-3 py-10 text-center"
      >
        <TriangleAlert aria-hidden size={24} className="text-muted" />
        <p className="font-semibold">Your PC could not be checked</p>
        <p className="max-w-xs text-body text-muted">{error}</p>
        <Button onClick={check}>Try again</Button>
      </Card>
    );
  }

  return (
    <>
      {error ? <Notice>{`Could not check again. ${error}`}</Notice> : null}
      <ScoreCard report={report} state={state} />
      {report.unchecked.length > 0 ? (
        <Notice>
          {`Volt could not check ${report.unchecked.map(checkName).join(', ')}. They are not part of the score.`}
        </Notice>
      ) : null}
    </>
  );
}
