import { HardDrive, type LucideIcon, MemoryStick, Rocket, Trash2, Zap } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import {
  Badge,
  Button,
  Card,
  ListGroup,
  ListRow,
  PageHeader,
  ProgressRing,
  useToast,
} from '../../shared/ui';
import {
  type SampleMetric,
  sampleHealth,
  sampleMetrics,
  sampleRecommendations,
} from './sampleData';

const metricIcons: Record<SampleMetric['id'], LucideIcon> = {
  memory: MemoryStick,
  junk: Trash2,
  startup: Rocket,
  disk: HardDrive,
};

/**
 * Home screen mock: health score, one Boost button, four metrics and a few recommendations.
 * Static sample data and no business logic. Boost only simulates a short wait.
 */
export function HomeScreen() {
  const toast = useToast();
  const [boosting, setBoosting] = useState(false);
  const timer = useRef<number | undefined>(undefined);

  useEffect(() => () => window.clearTimeout(timer.current), []);

  const boost = () => {
    setBoosting(true);
    timer.current = window.setTimeout(() => {
      setBoosting(false);
      toast.show({ message: 'Sample only: Boost would run here. Nothing was changed.' });
    }, 1400);
  };

  return (
    <>
      <PageHeader
        title="Home"
        description="Your PC at a glance."
        actions={<Badge>Sample data</Badge>}
      />
      <div className="flex flex-col gap-4">
        <Card
          as="section"
          aria-label="Health score"
          className="flex flex-col items-center gap-4 py-5 wide:flex-row wide:gap-8 wide:px-8 wide:py-6"
        >
          <ProgressRing value={sampleHealth.score} label="Health score" size={136} strokeWidth={10}>
            <div>
              <div className="text-display font-semibold tabular-nums">{sampleHealth.score}</div>
              <div className="text-body text-muted">{sampleHealth.label}</div>
            </div>
          </ProgressRing>
          <div className="flex w-full flex-col items-center gap-4 text-center wide:items-start wide:text-left">
            <div>
              <h2 className="text-subtitle font-semibold">Your PC is in good shape</h2>
              <p className="mt-1 text-body text-muted">
                3 things could make it faster and lighter.
              </p>
            </div>
            <Button
              variant="primary"
              size="lg"
              className="w-full wide:w-56"
              loading={boosting}
              icon={<Zap aria-hidden size={20} />}
              onClick={boost}
            >
              {boosting ? 'Boosting...' : 'Boost'}
            </Button>
          </div>
        </Card>

        <section aria-label="Metrics" className="grid grid-cols-2 gap-3 wide:grid-cols-4">
          {sampleMetrics.map((metric) => {
            const Icon = metricIcons[metric.id];
            return (
              <Card key={metric.id} className="flex flex-col gap-3 p-3.5">
                <div className="flex items-center gap-2 text-muted">
                  <Icon aria-hidden size={16} strokeWidth={1.75} />
                  <span className="text-body">{metric.label}</span>
                </div>
                <div className="text-title font-semibold tabular-nums">{metric.value}</div>
                <p className="-mt-2 text-caption text-muted">{metric.caption}</p>
              </Card>
            );
          })}
        </section>

        <section aria-labelledby="home-recommendations">
          <h2 id="home-recommendations" className="pb-2 text-subtitle font-semibold">
            Recommended for you
          </h2>
          <ListGroup>
            {sampleRecommendations.map((item) => (
              <ListRow
                key={item.id}
                title={item.title}
                description={item.description}
                badge={<Badge risk={item.risk} />}
                trailing={
                  <Button
                    size="sm"
                    aria-label={`Review: ${item.title}`}
                    onClick={() =>
                      toast.show({ message: 'Sample only: Review would open the matching screen.' })
                    }
                  >
                    Review
                  </Button>
                }
              />
            ))}
          </ListGroup>
        </section>
      </div>
    </>
  );
}
