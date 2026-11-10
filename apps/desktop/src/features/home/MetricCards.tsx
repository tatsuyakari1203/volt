import { Cpu, HardDrive, type LucideIcon, MemoryStick, Monitor, TriangleAlert } from 'lucide-react';
import { t } from '../../shared/i18n';
import { Button, Card, cx } from '../../shared/ui';
import { buildCards, type MetricCardId } from './cards';
import type { MetricsState } from './hooks';

const icons: Record<MetricCardId, LucideIcon> = {
  cpu: Cpu,
  memory: MemoryStick,
  gpu: Monitor,
  disk: HardDrive,
};

/** The cards before the first reading arrives, in the order they are shown. */
function placeholders(): ReadonlyArray<{ id: MetricCardId; label: string }> {
  return [
    { id: 'cpu', label: t('home.card.cpu') },
    { id: 'memory', label: t('home.card.memory') },
    { id: 'gpu', label: t('home.card.gpu') },
    { id: 'disk', label: t('home.card.disk') },
  ];
}

function CardFrame({
  id,
  label,
  value,
  lines,
  empty = false,
  busy = false,
}: {
  id: MetricCardId;
  label: string;
  value: string;
  lines: readonly string[];
  empty?: boolean;
  busy?: boolean;
}) {
  const Icon = icons[id];
  return (
    <Card as="article" aria-label={label} aria-busy={busy || undefined} className="p-3.5">
      <div className="flex items-center gap-2 text-muted">
        <Icon aria-hidden size={16} strokeWidth={1.75} />
        <span className="text-body">{label}</span>
      </div>
      <div className={cx('mt-3 text-title font-semibold tabular-nums', empty && 'text-muted')}>
        {value}
      </div>
      {lines.map((line) => (
        <p key={line} className="mt-0.5 break-words text-caption text-muted">
          {line}
        </p>
      ))}
    </Card>
  );
}

/**
 * The four live cards: CPU, memory, GPU and disk. While the first reading is on its way they
 * show a dash and "Waiting for the first reading"; if the shell cannot be reached the cards are
 * replaced by one message with a retry. A value the shell reports as `null` (CPU before its
 * second sample, a machine without a GPU) is an honest empty card, never a made-up number.
 */
export function MetricCards({
  state,
  systemMount,
  onRetry,
}: {
  state: MetricsState;
  /** The system disk, so the disk card shows the drive Windows lives on. */
  systemMount?: string;
  onRetry: () => void;
}) {
  if (state.status === 'error') {
    return (
      <section aria-label={t('home.metrics.label')}>
        <Card role="alert" className="flex flex-col items-center gap-3 py-8 text-center">
          <TriangleAlert aria-hidden size={24} className="text-muted" />
          <p className="font-semibold">{t('home.metrics.unavailable')}</p>
          <p className="max-w-xs text-body text-muted">{state.message}</p>
          <Button onClick={onRetry}>{t('common.tryAgain')}</Button>
        </Card>
      </section>
    );
  }

  return (
    <section
      aria-label={t('home.metrics.label')}
      className="grid grid-cols-2 gap-3 wide:grid-cols-4"
    >
      {state.status === 'loading'
        ? placeholders().map(({ id, label }) => (
            <CardFrame
              key={id}
              id={id}
              label={label}
              value="—"
              lines={[t('home.metrics.waiting')]}
              empty
              busy
            />
          ))
        : buildCards(state.metrics, systemMount).map((card) => (
            <CardFrame
              key={card.id}
              id={card.id}
              label={card.label}
              value={card.value}
              lines={card.lines}
              empty={card.empty}
            />
          ))}
    </section>
  );
}
