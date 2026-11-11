import { useEffect, useState } from 'react';
import { getMemoryHistory, type MemoryPointView, onMemoryTrend } from '../../shared/ipc';
import { mergePoints } from './trend';

/**
 * Follows memory by list while the Memory page is open. The listener is registered first and the
 * history is asked for after that, so a reading that arrives in between is not lost; the two are
 * merged by time. A failure (the shell is not reachable) leaves the chart empty rather than
 * breaking the page.
 */
export function useMemoryHistory(): MemoryPointView[] {
  const [points, setPoints] = useState<MemoryPointView[]>([]);

  useEffect(() => {
    let active = true;
    let stop: (() => void) | undefined;
    onMemoryTrend((point) => {
      if (active) setPoints((current) => mergePoints(current, [point]));
    })
      .then((unlisten) => {
        if (!active) {
          unlisten();
          return undefined;
        }
        stop = unlisten;
        return getMemoryHistory();
      })
      .then((history) => {
        if (active && history) setPoints((current) => mergePoints(history, current));
      })
      .catch(() => {
        // No chart without the shell; the rest of the page does not depend on it.
      });
    return () => {
      active = false;
      stop?.();
    };
  }, []);

  return points;
}
