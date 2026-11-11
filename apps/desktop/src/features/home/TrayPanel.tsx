import { Cpu, MemoryStick, Rocket, TriangleAlert } from 'lucide-react';
import { useCallback, useEffect, useRef, useState } from 'react';
import { t } from '../../shared/i18n';
import {
  closeTrayPanel,
  type HealthView,
  openMainFromPanel,
  runPanelBoost,
} from '../../shared/ipc';
import { describeError } from '../../shared/text/errors';
import { Button, Card, cx } from '../../shared/ui';
import { buildCards, type MetricCardSpec } from './cards';
import { type MetricsState, useHealthReport, useLiveMetrics } from './hooks';
import { levelLabel } from './messages';

/** What sits under the Boost button: the line the shell wrote, or why there is none. */
export interface BoostLine {
  text: string;
  failed: boolean;
}

/** One figure of the panel: CPU or memory, with the one line that explains it. */
function Figure({ card, icon }: { card: MetricCardSpec; icon: 'cpu' | 'memory' }) {
  const Icon = icon === 'cpu' ? Cpu : MemoryStick;
  return (
    <Card as="article" aria-label={card.label} className="p-3">
      <div className="flex items-center gap-2 text-muted">
        <Icon aria-hidden size={16} strokeWidth={1.75} />
        <span className="text-body">{card.label}</span>
      </div>
      <div className={cx('mt-2 text-title font-semibold tabular-nums', card.empty && 'text-muted')}>
        {card.value}
      </div>
      {card.lines.map((line) => (
        <p key={line} className="mt-0.5 break-words text-caption text-muted">
          {line}
        </p>
      ))}
    </Card>
  );
}

/**
 * The panel as it is drawn, from what it knows. Memory and CPU come from the live metrics, the
 * health score is shown when the check succeeded and left out when it did not, and Boost has its
 * result line under it. Nothing here talks to the shell: `TrayPanel` does.
 */
export function PanelView({
  metrics,
  health,
  boosting,
  line,
  onBoost,
  onOpen,
}: {
  metrics: MetricsState;
  health: HealthView | null;
  boosting: boolean;
  line: BoostLine | null;
  onBoost: () => void;
  onOpen: () => void;
}) {
  const cards = metrics.status === 'ready' ? buildCards(metrics.metrics) : [];
  const cpu = cards.find((card) => card.id === 'cpu');
  const memory = cards.find((card) => card.id === 'memory');
  const waiting = (label: string, lines: string[]): MetricCardSpec => ({
    id: 'cpu',
    label,
    value: '—',
    lines,
    empty: true,
  });
  const none = metrics.status === 'error' ? [t('home.metrics.unavailable')] : [];
  return (
    <main
      aria-label={t('panel.region')}
      className="flex h-screen flex-col gap-3 border border-line bg-canvas p-3 text-fg"
    >
      <div className="grid grid-cols-2 gap-3">
        <Figure card={cpu ?? waiting(t('home.card.cpu'), none)} icon="cpu" />
        <Figure card={memory ?? waiting(t('home.card.memory'), none)} icon="memory" />
      </div>
      {health ? (
        <p className="flex items-center justify-between px-1 text-body">
          <span className="text-muted">{t('home.score.label')}</span>
          <span className="font-semibold tabular-nums">
            {health.level === 'unknown'
              ? levelLabel(health.level)
              : `${health.score} · ${levelLabel(health.level)}`}
          </span>
        </p>
      ) : null}
      <Button
        variant="primary"
        size="lg"
        icon={<Rocket aria-hidden size={18} />}
        loading={boosting}
        onClick={onBoost}
        className="w-full"
      >
        {boosting ? t('home.boosting') : t('home.boost')}
      </Button>
      <div role="status" className="min-h-10 px-1 text-body">
        {line ? (
          <p className={cx('flex items-start gap-2', line.failed && 'text-caution-fg')}>
            {line.failed ? (
              <TriangleAlert aria-hidden size={16} className="mt-0.5 shrink-0" />
            ) : null}
            <span className="min-w-0 flex-1 break-words">{line.text}</span>
          </p>
        ) : null}
      </div>
      <Button onClick={onOpen} className="mt-auto w-full">
        {t('panel.openVolt')}
      </Button>
    </main>
  );
}

/**
 * The tray panel: live memory and CPU, the health score, Boost and a way to the full window.
 * Escape closes it, which also destroys its WebView; the shell closes it on focus loss.
 */
export function TrayPanel() {
  const metrics = useLiveMetrics();
  const health = useHealthReport();
  const [boosting, setBoosting] = useState(false);
  const [line, setLine] = useState<BoostLine | null>(null);
  const running = useRef(false);
  const alive = useRef(true);

  useEffect(() => {
    alive.current = true;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') void closeTrayPanel().catch(() => undefined);
    };
    window.addEventListener('keydown', onKey);
    return () => {
      alive.current = false;
      window.removeEventListener('keydown', onKey);
    };
  }, []);

  const boost = useCallback(() => {
    if (running.current) return;
    running.current = true;
    setBoosting(true);
    setLine(null);
    runPanelBoost()
      .then((result) => {
        if (!alive.current) return;
        setLine(
          result
            ? { text: result.line, failed: result.failed }
            : { text: t('panel.boost.busy'), failed: false },
        );
      })
      .catch((failure: unknown) => {
        if (alive.current) setLine({ text: describeError(failure), failed: true });
      })
      .finally(() => {
        running.current = false;
        if (alive.current) setBoosting(false);
      });
  }, []);

  const open = useCallback(() => {
    void openMainFromPanel().catch(() => undefined);
  }, []);

  return (
    <PanelView
      metrics={metrics}
      health={health.report}
      boosting={boosting}
      line={line}
      onBoost={boost}
      onOpen={open}
    />
  );
}
