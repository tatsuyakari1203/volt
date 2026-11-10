import { useCallback, useEffect, useRef, useState } from 'react';
import { type BoostResult, runBoost } from '../../shared/ipc';
import { describeError } from './messages';

export interface BoostState {
  running: boolean;
  report: BoostResult | null;
  error: string | null;
  run: () => void;
}

/**
 * Runs Boost and keeps its latest report. `incoming` is a report the shell ran itself (the tray
 * item): when a new one arrives it replaces the shown report.
 */
export function useBoost(incoming: BoostResult | null | undefined): BoostState {
  const [running, setRunning] = useState(false);
  const [report, setReport] = useState<BoostResult | null>(incoming ?? null);
  const [error, setError] = useState<string | null>(null);
  const alive = useRef(true);

  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  useEffect(() => {
    if (incoming) {
      setReport(incoming);
      setError(null);
    }
  }, [incoming]);

  const run = useCallback(() => {
    setRunning(true);
    setError(null);
    runBoost()
      .then((result) => {
        if (alive.current) setReport(result);
      })
      .catch((e: unknown) => {
        if (alive.current) setError(describeError(e));
      })
      .finally(() => {
        if (alive.current) setRunning(false);
      });
  }, []);

  return { running, report, error, run };
}
