import { Check, Clock, Globe, Loader2, Minus, RefreshCw, TriangleAlert } from 'lucide-react';
import { useCallback, useEffect, useRef, useState } from 'react';
import { say, t, tn } from '../../shared/i18n';
import {
  type AppUpdateView,
  checkAppUpdates,
  type UpdateCheckView,
  updateApp,
} from '../../shared/ipc';
import { describeError } from '../../shared/text/errors';
import { Button, Card, ListGroup } from '../../shared/ui';

type Step = 'waiting' | 'running' | 'updated' | 'restart' | 'failed' | 'cancelled';

interface Item {
  update: AppUpdateView;
  step: Step;
  reason: string | null;
}

type Phase =
  | { status: 'idle' }
  | { status: 'checking' }
  | { status: 'error'; message: string }
  | { status: 'ready'; check: UpdateCheckView };

const stepKeys = {
  waiting: 'apps.updates.queue.waiting',
  running: 'apps.updates.queue.running',
  updated: 'apps.updates.queue.updated',
  restart: 'apps.updates.queue.restart',
  cancelled: 'apps.updates.queue.cancelled',
} as const;

function stepIcon(step: Step) {
  switch (step) {
    case 'waiting':
      return <Clock aria-hidden size={16} className="text-muted" />;
    case 'running':
      return (
        <Loader2
          aria-hidden
          size={16}
          className="animate-spin text-accent-text motion-reduce:animate-none"
        />
      );
    case 'updated':
      return <Check aria-hidden size={16} className="text-accent-text" />;
    case 'cancelled':
      return <Minus aria-hidden size={16} className="text-muted" />;
    default:
      return <TriangleAlert aria-hidden size={16} />;
  }
}

function stepText(item: Item): string {
  if (item.step === 'failed') return t('apps.updates.queue.failed', { reason: item.reason ?? '' });
  return t(stepKeys[item.step]);
}

/**
 * App updates through winget. Nothing runs until the user presses "Check for updates", and the
 * card says first that this uses the network. The Rust core reads winget's table and decides how
 * each update ended; this page lists the updates, runs the chosen ones one after another (Stop
 * ends the queue after the one that is running) and shows a result for each.
 */
export function UpdatesPanel() {
  const [phase, setPhase] = useState<Phase>({ status: 'idle' });
  const [ticked, setTicked] = useState<ReadonlySet<string>>(new Set());
  const [items, setItems] = useState<Item[] | null>(null);
  const [running, setRunning] = useState(false);
  const [stopping, setStopping] = useState(false);
  const alive = useRef(true);
  const stopRequested = useRef(false);

  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
      stopRequested.current = true;
    };
  }, []);

  const check = useCallback(() => {
    setPhase({ status: 'checking' });
    setTicked(new Set());
    setItems(null);
    checkAppUpdates()
      .then((result) => {
        if (alive.current) setPhase({ status: 'ready', check: result });
      })
      .catch((error: unknown) => {
        if (alive.current) setPhase({ status: 'error', message: describeError(error) });
      });
  }, []);

  const patch = useCallback((id: string, change: Partial<Item>) => {
    if (!alive.current) return;
    setItems((current) =>
      current
        ? current.map((item) => (item.update.id === id ? { ...item, ...change } : item))
        : current,
    );
  }, []);

  const run = useCallback(
    async (updates: AppUpdateView[]) => {
      if (running || updates.length === 0) return;
      stopRequested.current = false;
      setStopping(false);
      setRunning(true);
      setItems(updates.map((update) => ({ update, step: 'waiting', reason: null })));
      for (const [index, update] of updates.entries()) {
        if (stopRequested.current) {
          if (alive.current) {
            const left = new Set(updates.slice(index).map((u) => u.id));
            setItems((current) =>
              current
                ? current.map((item) =>
                    left.has(item.update.id) ? { ...item, step: 'cancelled' } : item,
                  )
                : current,
            );
          }
          break;
        }
        patch(update.id, { step: 'running' });
        try {
          const result = await updateApp(update.id);
          if (result.outcome === 'failed') {
            patch(update.id, {
              step: 'failed',
              reason: say(result.reasonPhrase, result.reason ?? ''),
            });
          } else {
            patch(update.id, { step: result.outcome === 'updated' ? 'updated' : 'restart' });
          }
        } catch (error) {
          patch(update.id, { step: 'failed', reason: describeError(error) });
        }
      }
      if (alive.current) {
        setRunning(false);
        setStopping(false);
      }
    },
    [patch, running],
  );

  // Done: what was updated is no longer on offer.
  const finish = () => {
    const done = new Set(
      (items ?? [])
        .filter((item) => item.step === 'updated' || item.step === 'restart')
        .map((item) => item.update.id),
    );
    setPhase((current) =>
      current.status === 'ready'
        ? {
            status: 'ready',
            check: {
              ...current.check,
              updates: current.check.updates.filter((u) => !done.has(u.id)),
            },
          }
        : current,
    );
    setTicked(new Set());
    setItems(null);
  };

  const toggle = (id: string, on: boolean) =>
    setTicked((current) => {
      const next = new Set(current);
      if (on) next.add(id);
      else next.delete(id);
      return next;
    });

  const result = phase.status === 'ready' ? phase.check : null;
  const updates = result?.updates ?? [];
  const chosen = updates.filter((update) => ticked.has(update.id));
  const count = (step: Step) => items?.filter((item) => item.step === step).length ?? 0;
  const busy = running || phase.status === 'checking';

  return (
    <Card as="section" aria-label={t('apps.updates.title')} className="flex flex-col gap-3">
      <div className="flex items-start justify-between gap-3">
        <h2 className="font-semibold">{t('apps.updates.title')}</h2>
        <Button size="sm" disabled={busy} onClick={check}>
          {phase.status === 'ready' || phase.status === 'error'
            ? t('apps.updates.recheck')
            : t('apps.updates.check')}
        </Button>
      </div>
      <p className="flex items-start gap-2 text-caption text-muted">
        <Globe aria-hidden size={14} className="mt-0.5 shrink-0" />
        <span>{t('apps.updates.network')}</span>
      </p>

      {phase.status === 'checking' ? (
        <p role="status" className="flex items-center gap-2 text-muted">
          <RefreshCw aria-hidden size={16} className="animate-spin motion-reduce:animate-none" />
          {t('apps.updates.checking')}
        </p>
      ) : null}

      {phase.status === 'error' ? (
        <p role="alert" className="text-body">
          {t('apps.updates.error', { error: phase.message })}
        </p>
      ) : null}

      {result?.problem ? (
        <div
          role="alert"
          className="flex items-start gap-3 rounded-md bg-caution p-3 text-body text-caution-fg"
        >
          <TriangleAlert aria-hidden size={18} className="mt-0.5 shrink-0" />
          <div className="min-w-0 flex-1">
            <p className="font-semibold">{t('apps.updates.problem.title')}</p>
            <p>{say(result.problem.messagePhrase, result.problem.message)}</p>
            <p className="mt-1">{say(result.problem.fixPhrase, result.problem.fix)}</p>
          </div>
        </div>
      ) : null}

      {result && !result.problem ? (
        <>
          <p className="text-caption text-muted" aria-live="polite">
            {updates.length === 0
              ? t('apps.updates.none')
              : tn('apps.updates.found', updates.length)}
          </p>
          {result.skipped > 0 ? (
            <p className="text-caption text-muted">
              {t('apps.updates.skipped', { count: result.skipped })}
            </p>
          ) : null}
        </>
      ) : null}

      {updates.length > 0 && items === null ? (
        <>
          <ListGroup aria-label={t('apps.updates.list.label')}>
            {updates.map((update) => (
              <li key={update.id}>
                <label className="flex items-start gap-3 px-4 py-3">
                  <input
                    type="checkbox"
                    className="mt-1 size-4 shrink-0 accent-(--color-accent)"
                    aria-label={t('apps.updates.row.select', { name: update.name })}
                    checked={ticked.has(update.id)}
                    onChange={(event) => toggle(update.id, event.target.checked)}
                  />
                  <div className="min-w-0 flex-1">
                    <p className="wrap-anywhere font-semibold">{update.name}</p>
                    <p className="text-caption text-muted">
                      {t('apps.updates.row.versions', {
                        from: update.current,
                        to: update.available,
                      })}
                    </p>
                  </div>
                </label>
              </li>
            ))}
          </ListGroup>
          <p className="text-caption text-muted">{t('apps.updates.noUndo')}</p>
          <div className="flex flex-wrap gap-2">
            <Button
              variant="primary"
              disabled={chosen.length === 0 || busy}
              onClick={() => void run(chosen)}
            >
              {t('apps.updates.selected', { count: chosen.length })}
            </Button>
            <Button disabled={busy} onClick={() => void run(updates)}>
              {t('apps.updates.all', { count: updates.length })}
            </Button>
          </div>
        </>
      ) : null}

      {items ? (
        <>
          <h3 className="font-semibold">
            {running ? t('apps.updates.queue.title') : t('apps.updates.queue.title.done')}
          </h3>
          <ul aria-label={t('apps.updates.queue.label')} className="flex flex-col gap-2">
            {items.map((item) => (
              <li key={item.update.id} className="flex items-start gap-2">
                <span className="mt-0.5 shrink-0">{stepIcon(item.step)}</span>
                <div className="min-w-0 flex-1">
                  <p className="wrap-anywhere font-semibold">{item.update.name}</p>
                  <p
                    className="wrap-anywhere text-caption text-muted"
                    role={item.step === 'running' ? 'status' : undefined}
                  >
                    {stepText(item)}
                  </p>
                </div>
              </li>
            ))}
          </ul>
          <p className="text-caption text-muted">{t('apps.updates.noUndo')}</p>
          {running ? (
            <div className="flex justify-end">
              <Button
                size="sm"
                disabled={stopping}
                onClick={() => {
                  stopRequested.current = true;
                  setStopping(true);
                }}
              >
                {stopping ? t('apps.updates.queue.stopping') : t('apps.updates.queue.stop')}
              </Button>
            </div>
          ) : (
            <div className="flex items-center justify-between gap-3">
              <p className="text-caption text-muted">
                {t('apps.updates.queue.summary', {
                  updated: count('updated'),
                  restart: count('restart'),
                  failed: count('failed'),
                  cancelled: count('cancelled'),
                })}
              </p>
              <Button variant="primary" size="sm" onClick={finish}>
                {t('apps.updates.queue.done')}
              </Button>
            </div>
          )}
        </>
      ) : null}
    </Card>
  );
}
