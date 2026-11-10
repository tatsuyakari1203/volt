import { Cpu, HardDrive, type LucideIcon, MemoryStick, Monitor, TriangleAlert } from 'lucide-react';
import { Button, Card, cx } from '../../shared/ui';
import { buildCards, type MetricCardId } from './cards';
import type { MetricsState } from './hooks';

const icons: Record<MetricCardId, LucideIcon> = {
  cpu: Cpu,
  memory: MemoryStick,
  gpu: Monitor,
  disk: HardDrive,
};

const placeholders: ReadonlyArray<{ id: MetricCardId; label: string }> = [
  { id: 'cpu', label: 'CPU' },
  { id: 'memory', label: 'Memory' },
  { id: 'gpu', label: 'GPU' },
  { id: 'disk', label: 'Disk space' },
];

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
      <section aria-label="Live metrics">
        <Card role="alert" className="flex flex-col items-center gap-3 py-8 text-center">
          <TriangleAlert aria-hidden size={24} className="text-muted" />
          <p className="font-semibold">Live numbers are not available</p>
          <p className="max-w-xs text-body text-muted">{state.message}</p>
          <Button onClick={onRetry}>Try again</Button>
        </Card>
      </section>
    );
  }

  return (
    <section aria-label="Live metrics" className="grid grid-cols-2 gap-3 wide:grid-cols-4">
      {state.status === 'loading'
        ? placeholders.map(({ id, label }) => (
            <CardFrame
              key={id}
              id={id}
              label={label}
              value="—"
              lines={['Waiting for the first reading']}
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
