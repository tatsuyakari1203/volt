import { CircleCheck, TriangleAlert } from 'lucide-react';
import { t, tNodes, tn } from '../../shared/i18n';
import type { BoostResult } from '../../shared/ipc';
import { describeArea, formatChange, operationText, reasonText } from '../../shared/text/memory';
import { formatBytes, formatSize } from '../../shared/text/size';
import { Button, Card } from '../../shared/ui';

/** The sentence about what the read-only scan found. It never says anything was freed. */
export function junkText(junk: NonNullable<BoostResult['junk']>): string {
  if (junk.files === 0) return t('home.junk.none');
  return t(junk.truncated ? 'home.junk.foundAtLeast' : 'home.junk.found', {
    files: tn('count.file', junk.files),
    size: formatSize(junk.bytes),
  });
}

/** The sentence about what cleaning moved. Moved is not freed: the files still use their space. */
export function movedText(moved: NonNullable<BoostResult['movedToBin']>): string {
  return [
    t('home.moved.base', { files: tn('count.file', moved.files), size: formatSize(moved.bytes) }),
    moved.skipped > 0 ? t('home.moved.left', { count: moved.skipped }) : '',
    t('home.moved.footer'),
  ]
    .filter(Boolean)
    .join(' ');
}

/**
 * The before and after report of one Boost: available memory, what each memory operation did,
 * what was skipped and why, what could be cleaned and, only when cleaning ran, what was moved to
 * the Recycle Bin. `onOpenCleaner` leads to the Clean page, where cleaning shows a preview first.
 */
export function BoostReport({
  report,
  onOpenCleaner,
}: {
  report: BoostResult;
  onOpenCleaner: () => void;
}) {
  const { availableBefore, availableAfter, junk, movedToBin } = report;
  return (
    <Card as="section" aria-label={t('home.boostReport.title')} className="flex flex-col gap-3">
      <h2 className="text-subtitle font-semibold">{t('home.boostReport.title')}</h2>
      <div>
        <p className="text-caption text-muted">{t('home.boostReport.available')}</p>
        {availableBefore !== null && availableAfter !== null ? (
          <p className="text-body">
            {tNodes('home.boostReport.beforeAfter', {
              before: (
                <span className="font-semibold tabular-nums">{formatBytes(availableBefore)}</span>
              ),
              after: (
                <span className="font-semibold tabular-nums">{formatBytes(availableAfter)}</span>
              ),
              change: formatChange(availableBefore, availableAfter),
            })}
          </p>
        ) : (
          <p className="text-body text-muted">{t('home.boostReport.notMeasured')}</p>
        )}
      </div>
      {report.areas.length > 0 || report.skipped.length > 0 ? (
        <ul className="flex flex-col gap-1">
          {report.areas.map((result) => (
            <li key={result.area} className="flex items-start gap-2 text-body">
              {result.status === 'done' ? (
                <CircleCheck aria-hidden size={16} className="mt-0.5 shrink-0" />
              ) : (
                <TriangleAlert aria-hidden size={16} className="mt-0.5 shrink-0" />
              )}
              <span>
                <span className="font-semibold">
                  {t('home.boostReport.operation', { title: operationText(result.area).title })}
                </span>{' '}
                {describeArea(result)}
              </span>
            </li>
          ))}
          {report.skipped.map((skip) => (
            <li key={skip.area} className="flex items-start gap-2 text-body">
              <TriangleAlert aria-hidden size={16} className="mt-0.5 shrink-0" />
              <span>
                <span className="font-semibold">
                  {t('home.boostReport.operation', { title: operationText(skip.area).title })}
                </span>{' '}
                {t('home.boostReport.skipped')} {reasonText(skip.state, skip.reason)}
              </span>
            </li>
          ))}
        </ul>
      ) : null}
      {junk ? (
        <div className="flex flex-wrap items-center justify-between gap-2">
          <p className="min-w-0 flex-1 text-body">{junkText(junk)}</p>
          {junk.files > 0 ? (
            <Button size="sm" onClick={onOpenCleaner}>
              {t('home.fix.openCleaner')}
            </Button>
          ) : null}
        </div>
      ) : null}
      {movedToBin ? <p className="text-body">{movedText(movedToBin)}</p> : null}
      <p className="text-caption text-muted">
        {t('home.boostReport.footer')}
        {report.recorded ? '' : ` ${t('home.boostReport.notRecorded')}`}
      </p>
    </Card>
  );
}
