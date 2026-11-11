import { t, tn } from '../../shared/i18n';
import type { MemoryPointView } from '../../shared/ipc';
import { listRows } from '../../shared/text/memory';
import { formatBytes } from '../../shared/text/size';
import { Card } from '../../shared/ui';
import { buildLayers, coveredMinutes, type ListKey, rangeOf, SPAN_MS } from './trend';

const WIDTH = 300;
const HEIGHT = 100;

/** Fill of each stacked layer: the same colours as the swatches of the lists below it. */
const FILL: Record<ListKey, string> = {
  inUseBytes: 'fill-accent',
  standbyBytes: 'fill-accent-soft',
  modifiedBytes: 'fill-caution',
  freeBytes: 'fill-surface-sunken',
};

/**
 * Memory by list over the last minutes: in use, standby, modified and free stacked to the
 * installed amount. It is a still picture that redraws once a second: nothing moves by itself, so
 * reduced motion needs no special case. Colour is not the only cue (each layer has an outline and
 * the legend names it), and the same numbers are available as text: the picture has a one line
 * description and a table for screen readers.
 */
export function MemoryChart({ points }: { points: readonly MemoryPointView[] }) {
  const last = points[points.length - 1];
  const layers = buildLayers(points, WIDTH, HEIGHT);
  const minutes = coveredMinutes(points);
  const description = last
    ? t('memory.chart.alt', {
        span: tn('memory.chart.minutes', minutes),
        inUse: formatBytes(last.lists.inUseBytes),
        standby: formatBytes(last.lists.standbyBytes),
        modified: formatBytes(last.lists.modifiedBytes),
        free: formatBytes(last.lists.freeBytes),
      })
    : '';
  return (
    <Card as="section" aria-label={t('memory.chart.label')} className="flex flex-col gap-3">
      <div>
        <h2 className="text-subtitle font-semibold">{t('memory.chart.title')}</h2>
        <p className="text-caption text-muted">{t('memory.chart.subtitle')}</p>
      </div>
      {layers.length === 0 || !last ? (
        <p role="status" className="py-6 text-center text-body text-muted">
          {t('memory.chart.collecting')}
        </p>
      ) : (
        <>
          <div className="flex gap-2">
            <div
              aria-hidden
              className="flex flex-col justify-between text-caption text-muted tabular-nums"
            >
              <span>{formatBytes(last.lists.totalBytes)}</span>
              <span>0</span>
            </div>
            <div className="min-w-0 flex-1">
              <svg
                role="img"
                aria-label={description}
                viewBox={`0 0 ${WIDTH} ${HEIGHT}`}
                preserveAspectRatio="none"
                className="block h-28 w-full rounded-sm border border-line bg-surface"
              >
                {layers.map((layer) => (
                  <path key={layer.key} d={layer.area} className={FILL[layer.key]} />
                ))}
                {layers.map((layer) => (
                  <path
                    key={`${layer.key}-edge`}
                    d={layer.edge}
                    fill="none"
                    strokeWidth={1}
                    vectorEffect="non-scaling-stroke"
                    className="stroke-line-strong"
                  />
                ))}
              </svg>
              <div
                aria-hidden
                className="mt-1 flex justify-between text-caption text-muted tabular-nums"
              >
                <span>
                  {t('memory.chart.start', { span: tn('memory.chart.minutes', SPAN_MS / 60_000) })}
                </span>
                <span>{t('memory.chart.now')}</span>
              </div>
            </div>
          </div>
          <ul className="flex flex-wrap gap-x-4 gap-y-1 text-caption">
            {listRows.map((row) => (
              <li key={row.key} className="flex items-center gap-1.5">
                <span
                  aria-hidden
                  className={`size-3 shrink-0 rounded-sm border border-line ${row.swatch}`}
                />
                {t(row.label)}
              </li>
            ))}
          </ul>
          <table className="sr-only">
            <caption>{description}</caption>
            <thead>
              <tr>
                <th scope="col">{t('memory.report.list')}</th>
                <th scope="col">{t('memory.chart.now')}</th>
                <th scope="col">{t('memory.chart.lowest')}</th>
                <th scope="col">{t('memory.chart.highest')}</th>
              </tr>
            </thead>
            <tbody>
              {listRows.map((row) => {
                const { low, high } = rangeOf(points, row.key);
                return (
                  <tr key={row.key}>
                    <th scope="row">{t(row.label)}</th>
                    <td>{formatBytes(last.lists[row.key])}</td>
                    <td>{formatBytes(low)}</td>
                    <td>{formatBytes(high)}</td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </>
      )}
    </Card>
  );
}
