import { History, Loader2, TriangleAlert } from 'lucide-react';
import { useCallback, useEffect, useRef, useState } from 'react';
import {
  type ActivityItem,
  type ActivityStatus,
  IpcUnavailableError,
  listActivity,
} from '../../shared/ipc';
import { Button, Card, ListGroup, ListRow, PageHeader } from '../../shared/ui';

/** Records read per page. */
const PAGE_SIZE = 50;

const statusLabels: Record<ActivityStatus, string> = {
  applied: 'Applied',
  reverted: 'Undone',
  failed: 'Failed',
  rolledBack: 'Rolled back',
  other: 'Other',
};

type Load =
  | { status: 'loading' }
  | { status: 'error'; message: string }
  | { status: 'ready'; items: ActivityItem[]; nextCursor: string | null };

function describe(error: unknown): string {
  if (error instanceof IpcUnavailableError) {
    return 'The history is only available inside the Volt app.';
  }
  return error instanceof Error ? error.message : String(error);
}

/** The time as the user's locale writes it; the raw ISO text when it cannot be parsed. */
function formatTime(iso: string): string {
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? iso : date.toLocaleString();
}

/**
 * The Undo screen for now: a read-only list of what Volt did, newest first, read from the
 * activity log. Each line has the time, the outcome and a one-sentence summary. Undoing a single
 * line and exporting the list come later; a tweak is undone from the Tweaks screen.
 */
export function UndoScreen() {
  const [load, setLoad] = useState<Load>({ status: 'loading' });
  const [more, setMore] = useState<'idle' | 'loading' | 'error'>('idle');
  const alive = useRef(true);

  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  const read = useCallback(() => {
    setLoad({ status: 'loading' });
    listActivity(PAGE_SIZE)
      .then((page) => {
        if (alive.current) {
          setLoad({ status: 'ready', items: page.items, nextCursor: page.nextCursor });
        }
      })
      .catch((error: unknown) => {
        if (alive.current) setLoad({ status: 'error', message: describe(error) });
      });
  }, []);

  useEffect(read, [read]);

  const older = (cursor: string) => {
    setMore('loading');
    listActivity(PAGE_SIZE, cursor)
      .then((page) => {
        if (!alive.current) return;
        setLoad((current) =>
          current.status === 'ready'
            ? {
                status: 'ready',
                items: [...current.items, ...page.items],
                nextCursor: page.nextCursor,
              }
            : current,
        );
        setMore('idle');
      })
      .catch(() => {
        if (alive.current) setMore('error');
      });
  };

  const nextCursor = load.status === 'ready' ? load.nextCursor : null;

  return (
    <>
      <PageHeader title="Undo" description="What Volt changed on this PC, newest first." />
      <div className="flex flex-col gap-3">
        {load.status === 'loading' ? (
          <Card role="status" className="flex items-center justify-center gap-2 py-10 text-muted">
            <Loader2 aria-hidden size={18} className="animate-spin motion-reduce:animate-none" />
            <p>Reading the history...</p>
          </Card>
        ) : null}

        {load.status === 'error' ? (
          <Card role="alert" className="flex flex-col items-center gap-3 py-10 text-center">
            <TriangleAlert aria-hidden size={24} className="text-muted" />
            <p className="font-semibold">The history could not be loaded</p>
            <p className="max-w-xs text-body text-muted">{load.message}</p>
            <Button onClick={read}>Try again</Button>
          </Card>
        ) : null}

        {load.status === 'ready' && load.items.length === 0 ? (
          <Card className="flex flex-col items-center gap-2 py-10 text-center">
            <History aria-hidden size={24} className="text-muted" />
            <p className="font-semibold">Nothing yet</p>
            <p className="max-w-xs text-body text-muted">
              When Volt changes something, it is listed here. To undo a tweak, turn it off on the
              Tweaks screen.
            </p>
          </Card>
        ) : null}

        {load.status === 'ready' && load.items.length > 0 ? (
          <>
            <ListGroup aria-label="Recent activity">
              {load.items.map((item) => (
                <ListRow
                  key={item.id}
                  title={statusLabels[item.status]}
                  description={
                    <>
                      <time dateTime={item.time} className="block">
                        {formatTime(item.time)}
                      </time>
                      <span className="mt-0.5 block text-fg">{item.summary}</span>
                    </>
                  }
                />
              ))}
            </ListGroup>
            {nextCursor ? (
              <div className="flex flex-col items-center gap-2">
                {more === 'error' ? (
                  <p role="alert" className="text-body text-muted">
                    Older entries could not be loaded.
                  </p>
                ) : null}
                <Button loading={more === 'loading'} onClick={() => older(nextCursor)}>
                  Show older
                </Button>
              </div>
            ) : null}
          </>
        ) : null}
      </div>
    </>
  );
}
