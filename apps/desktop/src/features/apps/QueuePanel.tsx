import { Check, Clock, Loader2, Minus, TriangleAlert } from 'lucide-react';
import type { ReactNode } from 'react';
import { type MessageKey, t } from '../../shared/i18n';
import { Button, Card } from '../../shared/ui';
import { type ItemStatus, tally } from './apps';
import type { Queue, QueueItem } from './useUninstallQueue';

const icons: Record<ItemStatus, ReactNode> = {
  waiting: <Clock aria-hidden size={16} className="text-muted" />,
  running: (
    <Loader2
      aria-hidden
      size={16}
      className="animate-spin text-accent-text motion-reduce:animate-none"
    />
  ),
  removed: <Check aria-hidden size={16} className="text-accent-text" />,
  stillInstalled: <TriangleAlert aria-hidden size={16} />,
  failed: <TriangleAlert aria-hidden size={16} />,
  unknown: <TriangleAlert aria-hidden size={16} />,
  gone: <Minus aria-hidden size={16} className="text-muted" />,
  skipped: <Minus aria-hidden size={16} className="text-muted" />,
};

const statusKeys = {
  waiting: 'apps.queue.waiting',
  running: 'apps.queue.running',
  removed: 'apps.queue.removed',
  stillInstalled: 'apps.queue.stillInstalled',
  unknown: 'apps.queue.unknown',
  gone: 'apps.queue.gone',
  skipped: 'apps.queue.skipped',
} as const satisfies Partial<Record<ItemStatus, MessageKey>>;

function statusText(item: QueueItem): string {
  if (item.status === 'failed') {
    if (item.error) return t('apps.queue.error', { error: item.error });
    return item.source === 'packaged' ? t('apps.queue.failedPackaged') : t('apps.queue.failed');
  }
  return t(statusKeys[item.status]);
}

export interface QueuePanelProps {
  queue: Queue;
  onStop: () => void;
  onStopWaiting: () => void;
  onDone: () => void;
}

/**
 * The uninstall queue while it runs and after it ended: each app with what is happening to it or
 * how it ended, and while it runs two ways out: stop waiting for the uninstaller that is running
 * (the queue goes on with the next app) or stop after it (the rest is skipped); then Done.
 */
export function QueuePanel({ queue, onStop, onStopWaiting, onDone }: QueuePanelProps) {
  const counts = tally(queue.items);
  const waiting = queue.items.some((item) => item.status === 'running');
  return (
    <Card
      as="section"
      aria-label={t('apps.queue.title')}
      className="sticky bottom-3 z-10 flex flex-col gap-3 shadow-pop"
    >
      <div className="flex items-center justify-between gap-3">
        <h2 className="font-semibold">
          {queue.running ? t('apps.queue.title') : t('apps.queue.title.done')}
        </h2>
        <p className="text-caption text-muted" aria-live="polite">
          {t('apps.queue.progress', { done: counts.finished, total: queue.items.length })}
        </p>
      </div>
      <ul
        aria-label={t('apps.queue.list.label')}
        className="flex max-h-56 flex-col gap-2 overflow-y-auto"
      >
        {queue.items.map((item) => (
          <li key={item.id} className="flex items-start gap-2">
            <span className="mt-0.5 shrink-0">{icons[item.status]}</span>
            <div className="min-w-0 flex-1">
              <p className="wrap-anywhere font-semibold">{item.name}</p>
              <p
                className="text-caption text-muted"
                role={item.status === 'running' ? 'status' : undefined}
              >
                {statusText(item)}
              </p>
            </div>
          </li>
        ))}
      </ul>
      <p className="text-caption text-muted">{t('apps.noUndo')}</p>
      {queue.running ? (
        <div className="flex flex-wrap justify-end gap-2">
          {waiting ? (
            <Button size="sm" disabled={queue.waitStopped} onClick={onStopWaiting}>
              {queue.waitStopped ? t('apps.queue.waitStopped') : t('apps.queue.stopWaiting')}
            </Button>
          ) : null}
          <Button size="sm" disabled={queue.stopping} onClick={onStop}>
            {queue.stopping ? t('apps.queue.stopping') : t('apps.queue.stop')}
          </Button>
        </div>
      ) : (
        <div className="flex items-center justify-between gap-3">
          <p className="text-caption text-muted">
            {t('apps.queue.summary', {
              removed: counts.removed,
              still: counts.still,
              failed: counts.failed,
              skipped: counts.skipped,
            })}
          </p>
          <Button variant="primary" size="sm" onClick={onDone}>
            {t('apps.queue.done')}
          </Button>
        </div>
      )}
    </Card>
  );
}
