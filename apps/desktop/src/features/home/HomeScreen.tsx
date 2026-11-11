import { Rocket, TriangleAlert } from 'lucide-react';
import { t } from '../../shared/i18n';
import type { BoostResult } from '../../shared/ipc';
import { Button, PageHeader } from '../../shared/ui';
import { BoostReport } from './BoostReport';
import { DriftNotice } from './DriftNotice';
import { Deductions, HealthScore } from './HealthSection';
import { useHealthReport, useLiveMetrics } from './hooks';
import { MetricCards } from './MetricCards';
import type { HomeTarget } from './messages';
import { useBoost } from './useBoost';

export type { HomeTarget } from './messages';

/**
 * Home: how the PC is doing and what to do about it. A health score that lists every point it
 * took off, each with a button to the screen that fixes it, and four live cards (CPU, memory,
 * GPU, disk). The score is worked out when the screen opens and when the user asks to check
 * again; the cards follow the live metrics. The Boost button runs the default boost with one click
 * and shows a before and after report; `trayReport` is the report of a Boost the tray ran.
 *
 * The screen does not know the shell's sections: `onNavigate` receives where the user wants to
 * go, and the shell decides how.
 */
export function HomeScreen({
  onNavigate,
  trayReport,
}: {
  onNavigate: (target: HomeTarget) => void;
  trayReport?: BoostResult | null;
}) {
  const health = useHealthReport();
  const metrics = useLiveMetrics();
  const boost = useBoost(trayReport);

  return (
    <>
      <PageHeader
        title={t('nav.home')}
        description={t('home.description')}
        actions={
          <Button
            variant="primary"
            icon={<Rocket aria-hidden size={16} />}
            loading={boost.running}
            onClick={boost.run}
          >
            {boost.running ? t('home.boosting') : t('home.boost')}
          </Button>
        }
      />
      <div className="flex flex-col gap-4">
        {boost.error ? (
          <div
            role="alert"
            className="flex items-start gap-3 rounded-md bg-caution p-3 text-body text-caution-fg"
          >
            <TriangleAlert aria-hidden size={18} className="mt-0.5 shrink-0" />
            <p className="min-w-0 flex-1">{boost.error}</p>
          </div>
        ) : null}
        {boost.report ? (
          <BoostReport report={boost.report} onOpenCleaner={() => onNavigate('clean')} />
        ) : null}
        <DriftNotice onReview={() => onNavigate('tweaks')} />
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
