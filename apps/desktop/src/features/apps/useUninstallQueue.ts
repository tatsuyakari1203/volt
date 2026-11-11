import { useCallback, useEffect, useRef, useState } from 'react';
import {
  type AppListView,
  type AppSourceView,
  IpcCommandError,
  stopWaitingForUninstaller,
  uninstallApp,
} from '../../shared/ipc';
import { describeError } from '../../shared/text/errors';
import { type ItemStatus, statusOf } from './apps';
import { forgetQueue, rememberQueue } from './queueMemory';

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
  /** Whether "Stop waiting" was pressed for the uninstaller that is running. */
  waitStopped: boolean;
}

/**
 * Runs the uninstall queue: one app at a time, in order. Each `uninstallApp` call starts the app's
 * own uninstaller and returns when it and everything it started have exited and the list has been
 * read again, so the next app starts only then. `stopWaiting` gives up waiting for the running
 * one (it keeps running) and the queue goes on. `stop` ends the queue after the running one; the
 * uninstaller that is running is never interrupted.
 *
 * The queue lives in this page. Leaving the page, or closing the window, ends it after the app
 * that is running; the apps that did not run are remembered (see `queueMemory`) and the next open
 * of the page names the ones that are still installed. The list that comes back with each result
 * goes to `onListing`.
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
    async (
      apps: ReadonlyArray<{ id: string; name: string; source: AppSourceView }>,
      includeSystem: boolean,
    ) => {
      if (busy.current || apps.length === 0) return;
      busy.current = true;
      stopRequested.current = false;
      setQueue({
        items: apps.map((app) => ({ ...app, status: 'waiting', error: null })),
        running: true,
        stopping: false,
        waitStopped: false,
      });
      // How many apps have finished, so a closed window can say what was left.
      let next = 0;
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
        rememberQueue(apps.slice(index));
        if (alive.current) {
          setQueue((current) => (current ? { ...current, waitStopped: false } : current));
        }
        patch(index, { status: 'running' });
        try {
          const result = await uninstallApp(app.id, includeSystem);
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
        next = index + 1;
      }
      busy.current = false;
      if (alive.current) {
        forgetQueue();
        setQueue((current) =>
          current ? { ...current, running: false, stopping: false } : current,
        );
      } else {
        rememberQueue(apps.slice(next));
      }
    },
    [patch],
  );

  const stop = useCallback(() => {
    stopRequested.current = true;
    setQueue((current) => (current?.running ? { ...current, stopping: true } : current));
  }, []);

  const stopWaiting = useCallback(() => {
    setQueue((current) => (current?.running ? { ...current, waitStopped: true } : current));
    void stopWaitingForUninstaller().catch(() => {});
  }, []);

  const dismiss = useCallback(() => setQueue(null), []);

  return { queue, start, stop, stopWaiting, dismiss };
}
