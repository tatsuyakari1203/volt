import { Ban, Check, Layers, TriangleAlert, Undo2 } from 'lucide-react';
import type { ReactNode } from 'react';
import { type MessageKey, say, t, tNodes } from '../../shared/i18n';
import type { ActivityItem, ChangeState } from '../../shared/ipc';
import { Button, cx, ListRow } from '../../shared/ui';
import type { HistoryEntry } from './history';
import { activityTitle, formatTime, sourceLabel, statusLabel } from './messages';

interface StateSpec {
  label: MessageKey;
  icon: ReactNode;
  className: string;
}

const NEUTRAL = 'border border-line bg-surface-sunken text-muted';

/** The badge of a row's undo state. A row that records an undo (`restored`) has none. */
const stateSpecs: Record<ChangeState, StateSpec | null> = {
  canUndo: {
    label: 'undo.state.canUndo',
    icon: <Undo2 aria-hidden size={12} />,
    className: 'bg-safe text-safe-fg',
  },
  undone: { label: 'undo.state.undone', icon: <Check aria-hidden size={12} />, className: NEUTRAL },
  failed: {
    label: 'undo.state.failed',
    icon: <TriangleAlert aria-hidden size={12} />,
    className: 'bg-caution text-caution-fg',
  },
  cannotUndo: {
    label: 'undo.state.cannotUndo',
    icon: <Ban aria-hidden size={12} />,
    className: NEUTRAL,
  },
  restored: null,
};

/** Small pill naming whether a change can be undone, was undone, failed or cannot be undone. */
function StateBadge({ state }: { state: ChangeState }) {
  const spec = stateSpecs[state];
  if (!spec) return null;
  return (
    <span
      className={cx(
        'inline-flex items-center gap-1 rounded-sm px-1.5 py-px text-caption font-semibold',
        spec.className,
      )}
    >
      {spec.icon}
      {t(spec.label)}
    </span>
  );
}

export interface HistoryListProps {
  /** The list, grouped by batch. */
  entries: readonly HistoryEntry[];
  /** What is being undone right now (`entry:<id>` or `batch:<id>`), or `null`. Blocks other undos. */
  busy: string | null;
  /** Undo one change. */
  onUndoEntry: (item: ActivityItem) => void;
  /** Ask to undo a whole batch (the screen confirms first). */
  onUndoBatch: (batch: string, title: string) => void;
}

/**
 * The history: every record newest first with its time, source, what changed, the previous value
 * and its undo state. Changes made together are shown as one group with an "Undo all" button.
 */
export function HistoryList({ entries, busy, onUndoEntry, onUndoBatch }: HistoryListProps) {
  return (
    <ul
      aria-label={t('undo.list.label')}
      className="divide-y divide-line overflow-hidden rounded-md border border-line bg-surface shadow-card"
    >
      {entries.map((entry) =>
        entry.kind === 'row' ? (
          <Row key={entry.item.id} item={entry.item} busy={busy} onUndo={onUndoEntry} showTitle />
        ) : (
          <li key={`batch-${entry.batch}-${entry.items[0]?.id}`}>
            <div className="flex items-center gap-3 bg-surface-sunken px-4 py-2.5">
              <Layers aria-hidden size={16} className="shrink-0 text-muted" />
              <div className="min-w-0 flex-1">
                <p className="font-semibold">{entry.title}</p>
                <p className="text-caption text-muted">
                  {t('undo.batch.records', { count: entry.items.length })}
                </p>
              </div>
              {entry.items.some((item) => item.state === 'canUndo') ? (
                <Button
                  size="sm"
                  aria-label={t('undo.batch.undoAll.label', { title: entry.title })}
                  loading={busy === `batch:${entry.batch}`}
                  disabled={busy !== null && busy !== `batch:${entry.batch}`}
                  onClick={() => onUndoBatch(entry.batch, entry.title)}
                >
                  {t('undo.batch.undoAll')}
                </Button>
              ) : null}
            </div>
            <ul
              aria-label={t('undo.batch.records.label', { title: entry.title })}
              className="divide-y divide-line border-t border-line"
            >
              {entry.items.map((item) => (
                <Row key={item.id} item={item} busy={busy} onUndo={onUndoEntry} />
              ))}
            </ul>
          </li>
        ),
      )}
    </ul>
  );
}

function Row({
  item,
  busy,
  onUndo,
  showTitle = false,
}: {
  item: ActivityItem;
  busy: string | null;
  onUndo: (item: ActivityItem) => void;
  /** Name what the record is about. A row inside a batch is named by the batch heading. */
  showTitle?: boolean;
}) {
  const key = item.entry === null ? null : `entry:${item.entry}`;
  return (
    <ListRow
      title={
        showTitle && activityTitle(item) !== null
          ? t('undo.row.titled', {
              title: activityTitle(item) ?? '',
              status: statusLabel(item.status),
            })
          : statusLabel(item.status)
      }
      badge={<StateBadge state={item.state} />}
      description={
        <>
          <span className="block">
            <time dateTime={item.time}>{formatTime(item.time)}</time>
            {` · ${sourceLabel(item.source)}`}
          </span>
          <span className="mt-0.5 block break-words text-fg">
            {say(item.summaryPhrase, item.summary)}
          </span>
          {item.previous !== null ? (
            <span className="mt-0.5 block break-words">
              {tNodes('undo.row.previous', {
                value: (
                  <span className="font-semibold text-fg">
                    {say(item.previousPhrase, item.previous)}
                  </span>
                ),
              })}
            </span>
          ) : null}
          {item.reason !== null ? (
            <span className="mt-0.5 block">{say(item.reasonPhrase, item.reason)}</span>
          ) : null}
        </>
      }
      trailing={
        item.state === 'canUndo' && key !== null ? (
          <Button
            size="sm"
            aria-label={t('undo.row.undo.label', {
              summary: say(item.summaryPhrase, item.summary),
            })}
            loading={busy === key}
            disabled={busy !== null && busy !== key}
            onClick={() => onUndo(item)}
          >
            {t('common.undo')}
          </Button>
        ) : null
      }
    />
  );
}
