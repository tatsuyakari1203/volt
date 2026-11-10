import { useCallback, useEffect, useRef, useState } from 'react';
import {
  getHealthReport,
  getLatestMetrics,
  type HealthView,
  type MetricsView,
  onMetricsUpdated,
} from '../../shared/ipc';
import { describeError } from './messages';

/** The health report as the screen needs it. */
export interface HealthState {
  /** The last report, kept on screen while a new check runs or fails. */
  report: HealthView | null;
  /** A check is running. */
  checking: boolean;
  /** Why the last check failed; cleared when a check starts. */
  error: string | null;
  /** Checks the machine again. Ignored while a check is running. */
  check: () => void;
}

/**
 * Reads the health report when the screen opens and whenever `check` is called. The score is
 * computed on demand and not on a timer: memory pressure is volatile, and a number that moved
 * while the user read it would look like a fault.
 */
export function useHealthReport(): HealthState {
  const [report, setReport] = useState<HealthView | null>(null);
  const [checking, setChecking] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const alive = useRef(true);
  const running = useRef(false);

  const check = useCallback(() => {
    if (running.current) return;
    running.current = true;
    setChecking(true);
    setError(null);
    getHealthReport()
      .then((next) => {
        if (alive.current) setReport(next);
      })
      .catch((failure: unknown) => {
        if (alive.current) setError(describeError(failure));
      })
      .finally(() => {
        running.current = false;
        if (alive.current) setChecking(false);
      });
  }, []);

  useEffect(() => {
    alive.current = true;
    check();
    return () => {
      alive.current = false;
    };
  }, [check]);

  return { report, checking, error, check };
}

/** The live readings as the screen needs them. */
export type MetricsState =
  | { status: 'loading' }
  | { status: 'error'; message: string }
  | { status: 'ready'; metrics: MetricsView };

/**
 * Follows the live metrics. The listener is registered first and the latest reading is asked for
 * after that: the other order would lose a reading that arrives in between. A reading that came
 * from the event is newer than the one the getter returns, so the getter never overwrites it.
 *
 * `retry` registers again after a failure (for example the shell was not reachable).
 */
export function useLiveMetrics(): MetricsState & { retry: () => void } {
  const [state, setState] = useState<MetricsState>({ status: 'loading' });
  const [attempt, setAttempt] = useState(0);

  // biome-ignore lint/correctness/useExhaustiveDependencies: `attempt` only re-runs the effect
  useEffect(() => {
    let active = true;
    let received = false;
    let stop: (() => void) | undefined;
    setState({ status: 'loading' });

    onMetricsUpdated((metrics) => {
      if (!active) return;
      received = true;
      setState({ status: 'ready', metrics });
    })
      .then((unlisten) => {
        if (!active) {
          unlisten();
          return undefined;
        }
        stop = unlisten;
        return getLatestMetrics();
      })
      .then((latest) => {
        if (active && !received && latest) {
          setState({ status: 'ready', metrics: latest });
        }
      })
      .catch((failure: unknown) => {
        // A reading that arrived in the meantime wins over the failure.
        if (active && !received) setState({ status: 'error', message: describeError(failure) });
      });

    return () => {
      active = false;
      stop?.();
    };
  }, [attempt]);

  const retry = useCallback(() => setAttempt((current) => current + 1), []);
  return { ...state, retry };
}
