import { useCallback, useEffect, useRef, useState } from 'react';
import {
  type AppListView,
  type AppSourceView,
  IpcCommandError,
  uninstallApp,
} from '../../shared/ipc';
import { describeError } from '../../shared/text/errors';
import { type ItemStatus, statusOf } from './apps';

/** One app of the queue. */
export interface QueueItem {
  id: string;
  name: string;
  source: AppSourceView;
  status: ItemStatus;
  /** The reason a command was refused or failed, when there is one. */
  error: string | null;
}

/** The queue as the screen draws it. */
export interface Queue {
  items: QueueItem[];
  /** Whether an uninstaller is running or still to run. */
  running: boolean;
  /** Whether Stop was pressed: the queue ends after the app that is running. */
  stopping: boolean;
}

/**
 * Runs the uninstall queue: one app at a time, in order. Each `uninstallApp` call starts the app's
 * own uninstaller and returns when it has exited and the list has been read again, so the next
 * app starts only then. `stop` ends the queue after the running one; the uninstaller that is
 * running is never interrupted. Leaving the screen also ends it after the running one. The list
 * that comes back with each result goes to `onListing`.
 */
export function useUninstallQueue(onListing: (listing: AppListView) => void) {
  const [queue, setQueue] = useState<Queue | null>(null);
  const alive = useRef(true);
  const stopRequested = useRef(false);
  const busy = useRef(false);
  const listing = useRef(onListing);
  listing.current = onListing;

  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
      stopRequested.current = true;
    };
  }, []);

  const patch = useCallback((index: number, change: Partial<QueueItem>) => {
    if (!alive.current) return;
    setQueue((current) =>
      current
        ? {
            ...current,
            items: current.items.map((item, i) => (i === index ? { ...item, ...change } : item)),
          }
        : current,
    );
  }, []);

  const start = useCallback(
    async (apps: ReadonlyArray<{ id: string; name: string; source: AppSourceView }>) => {
      if (busy.current || apps.length === 0) return;
      busy.current = true;
      stopRequested.current = false;
      setQueue({
        items: apps.map((app) => ({ ...app, status: 'waiting', error: null })),
        running: true,
        stopping: false,
      });
      for (const [index, app] of apps.entries()) {
        if (stopRequested.current) {
          if (alive.current) {
            setQueue((current) =>
              current
                ? {
                    ...current,
                    items: current.items.map((item, i) =>
                      i >= index ? { ...item, status: 'skipped' } : item,
                    ),
                  }
                : current,
            );
          }
          break;
        }
        patch(index, { status: 'running' });
        try {
          const result = await uninstallApp(app.id);
          if (alive.current) listing.current(result.listing);
          patch(index, { status: statusOf(result.outcome) });
        } catch (error) {
          patch(
            index,
            error instanceof IpcCommandError && error.error.kind === 'notFound'
              ? { status: 'gone' }
              : { status: 'failed', error: describeError(error) },
          );
        }
      }
      busy.current = false;
      if (alive.current) {
        setQueue((current) =>
          current ? { ...current, running: false, stopping: false } : current,
        );
      }
    },
    [patch],
  );

  const stop = useCallback(() => {
    stopRequested.current = true;
    setQueue((current) => (current?.running ? { ...current, stopping: true } : current));
  }, []);

  const dismiss = useCallback(() => setQueue(null), []);

  return { queue, start, stop, dismiss };
}
