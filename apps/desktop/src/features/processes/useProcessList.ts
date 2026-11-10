import { useCallback, useEffect, useRef, useState } from 'react';
import { IpcUnavailableError, listProcesses, type ProcessListView } from '../../shared/ipc';
import { describeError } from '../../shared/text/memory';

export type ProcessLoad =
  | { status: 'loading' }
  | { status: 'error'; message: string }
  | { status: 'ready'; list: ProcessListView; problem: string | null };

/** The gap between two readings. The first follows sooner, so CPU shows within a second. */
const FIRST_DELAY_MS = 1000;
const REFRESH_MS = 2000;

/**
 * Reads the process list about every two seconds while the page that uses it is mounted and the
 * window is visible. A reading never starts before the last one ended, so a slow machine is not
 * flooded. A failed reading keeps the list on screen and reports the problem; outside the Volt
 * window (no bridge) polling stops. `refresh` reads again now, for example after ending an app.
 */
export function useProcessList(): { load: ProcessLoad; refresh: () => void } {
  const [load, setLoad] = useState<ProcessLoad>({ status: 'loading' });
  const readNow = useRef<() => void>(() => {});

  useEffect(() => {
    let stopped = false;
    let inFlight = false;
    let timer: number | undefined;
    let delay = FIRST_DELAY_MS;

    const hidden = () => document.visibilityState === 'hidden';
    const schedule = () => {
      window.clearTimeout(timer);
      if (stopped || hidden()) return;
      timer = window.setTimeout(read, delay);
      delay = REFRESH_MS;
    };
    const read = () => {
      window.clearTimeout(timer);
      if (stopped || inFlight) return;
      inFlight = true;
      listProcesses()
        .then((list) => {
          if (!stopped) setLoad({ status: 'ready', list, problem: null });
          return true;
        })
        .catch((error: unknown) => {
          if (stopped) return false;
          const message = describeError(error);
          setLoad((current) =>
            current.status === 'ready'
              ? { ...current, problem: message }
              : { status: 'error', message },
          );
          // No bridge outside the Volt window: asking again changes nothing.
          return !(error instanceof IpcUnavailableError);
        })
        .then((again) => {
          inFlight = false;
          if (again) schedule();
        });
    };
    const onVisibility = () => {
      if (hidden()) window.clearTimeout(timer);
      else read();
    };

    readNow.current = read;
    document.addEventListener('visibilitychange', onVisibility);
    read();
    return () => {
      stopped = true;
      window.clearTimeout(timer);
      document.removeEventListener('visibilitychange', onVisibility);
    };
  }, []);

  const refresh = useCallback(() => readNow.current(), []);
  return { load, refresh };
}
