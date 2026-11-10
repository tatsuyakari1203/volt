import { CircleCheck, TriangleAlert } from 'lucide-react';
import type { BoostResult } from '../../shared/ipc';
import { describeArea, formatChange, operationText } from '../../shared/text/memory';
import { formatBytes, formatSize } from '../../shared/text/size';
import { Button, Card } from '../../shared/ui';

function filesText(count: number): string {
  return `${count.toLocaleString('en-US')} ${count === 1 ? 'file' : 'files'}`;
}

/** The sentence about what the read-only scan found. It never says anything was freed. */
export function junkText(junk: NonNullable<BoostResult['junk']>): string {
  if (junk.files === 0) return 'Nothing to clean was found.';
  return `${junk.truncated ? 'At least ' : ''}${filesText(junk.files)} (${formatSize(junk.bytes)}) could be moved to the Recycle Bin. Nothing was cleaned.`;
}

/** The sentence about what cleaning moved. Moved is not freed: the files still use their space. */
export function movedText(moved: NonNullable<BoostResult['movedToBin']>): string {
  const left = moved.skipped > 0 ? ` ${moved.skipped} in use or changed were left in place.` : '';
  return `Moved ${filesText(moved.files)} (${formatSize(moved.bytes)}) of temporary files to the Recycle Bin.${left} Disk space is not freed until the Recycle Bin is emptied.`;
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
    <Card as="section" aria-label="Boost report" className="flex flex-col gap-3">
      <h2 className="text-subtitle font-semibold">Boost report</h2>
      <div>
        <p className="text-caption text-muted">Memory available (free plus standby)</p>
        {availableBefore !== null && availableAfter !== null ? (
          <p className="text-body">
            <span className="font-semibold tabular-nums">{formatBytes(availableBefore)}</span>{' '}
            before,{' '}
            <span className="font-semibold tabular-nums">{formatBytes(availableAfter)}</span> after
            ({formatChange(availableBefore, availableAfter)})
          </p>
        ) : (
          <p className="text-body text-muted">
            Memory was not measured, because no memory operation ran or it could not be read.
          </p>
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
                <span className="font-semibold">{operationText[result.area].title}.</span>{' '}
                {describeArea(result)}
              </span>
            </li>
          ))}
          {report.skipped.map((skip) => (
            <li key={skip.area} className="flex items-start gap-2 text-body">
              <TriangleAlert aria-hidden size={16} className="mt-0.5 shrink-0" />
              <span>
                <span className="font-semibold">{operationText[skip.area].title}.</span> Skipped.{' '}
                {skip.reason}
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
              Open Cleaner
            </Button>
          ) : null}
        </div>
      ) : null}
      {movedToBin ? <p className="text-body">{movedText(movedToBin)}</p> : null}
      <p className="text-caption text-muted">
        Memory operations cannot be undone, and there is nothing to restore: Windows reloads what it
        needs.
        {report.recorded ? '' : ' This run could not be written to the activity history.'}
      </p>
    </Card>
  );
}
