import { PageHeader } from '../../shared/ui';
import { Deductions, HealthScore } from './HealthSection';
import { useHealthReport, useLiveMetrics } from './hooks';
import { MetricCards } from './MetricCards';
import type { HomeTarget } from './messages';

export type { HomeTarget } from './messages';

/**
 * Home: how the PC is doing and what to do about it. A health score that lists every point it
 * took off, each with a button to the screen that fixes it, and four live cards (CPU, memory,
 * GPU, disk). The score is worked out when the screen opens and when the user asks to check
 * again; the cards follow the live metrics. There is no Boost button yet: Boost has no engine,
 * and a button that does nothing is not shipped.
 *
 * The screen does not know the shell's sections: `onNavigate` receives where the user wants to
 * go, and the shell decides how.
 */
export function HomeScreen({ onNavigate }: { onNavigate: (target: HomeTarget) => void }) {
  const health = useHealthReport();
  const metrics = useLiveMetrics();

  return (
    <>
      <PageHeader title="Home" description="Your PC at a glance." />
      <div className="flex flex-col gap-4">
        <HealthScore state={health} />
        <MetricCards
          state={metrics}
          systemMount={health.report?.systemMount}
          onRetry={metrics.retry}
        />
        <Deductions report={health.report} onNavigate={onNavigate} />
      </div>
    </>
  );
}
