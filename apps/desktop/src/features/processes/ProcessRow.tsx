import { Lock } from 'lucide-react';
import { memo } from 'react';
import type { ProcessGroupView } from '../../shared/ipc';
import { Badge, Button } from '../../shared/ui';
import { cpuText, memoryText, processesText } from './processes';

export interface ProcessRowProps {
  group: ProcessGroupView;
  /** Ask to end the app (the screen confirms first). Must be a stable function. */
  onEnd: (group: ProcessGroupView) => void;
}

const protectedReason = {
  protectedWindows: 'Windows needs this to run, so Volt never ends it.',
  protectedVolt: 'This is Volt itself, so it cannot be ended here.',
} as const;

/**
 * Only what a row shows decides whether it draws again: a new reading changes the numbers of a few
 * rows, so most rows of a list of hundreds are left alone. Rows are keyed by the app id, so a
 * reading never mounts a row twice.
 */
function sameRow(a: ProcessRowProps, b: ProcessRowProps): boolean {
  return (
    a.onEnd === b.onEnd &&
    a.group.id === b.group.id &&
    a.group.name === b.group.name &&
    a.group.processCount === b.group.processCount &&
    a.group.endCount === b.group.endCount &&
    a.group.endability === b.group.endability &&
    memoryText(a.group.memoryBytes) === memoryText(b.group.memoryBytes) &&
    cpuText(a.group.cpuPercent) === cpuText(b.group.cpuPercent)
  );
}

/** One app: name and process count, summed memory and CPU, and End (or why it cannot be ended). */
export const ProcessRow = memo(function ProcessRow({ group, onEnd }: ProcessRowProps) {
  const { endability } = group;
  const locked = endability === 'protectedWindows' || endability === 'protectedVolt';
  return (
    <li className="flex items-center gap-3 px-4 py-2">
      <div className="min-w-0 flex-1">
        <p className="truncate font-semibold" title={group.path ?? group.name}>
          {group.name}
        </p>
        <p className="text-caption text-muted">{processesText(group.processCount)}</p>
      </div>
      <p className="w-16 shrink-0 text-right tabular-nums">
        <span className="sr-only">Memory </span>
        {memoryText(group.memoryBytes)}
      </p>
      <p className="w-12 shrink-0 text-right tabular-nums">
        <span className="sr-only">CPU </span>
        {cpuText(group.cpuPercent)}
      </p>
      <div className="flex w-20 shrink-0 justify-end">
        {locked ? (
          <span title={protectedReason[endability]}>
            <Badge>
              <Lock aria-hidden size={12} />
              Protected
            </Badge>
            <span className="sr-only">{protectedReason[endability]}</span>
          </span>
        ) : (
          <Button size="sm" aria-label={`End ${group.name}`} onClick={() => onEnd(group)}>
            End
          </Button>
        )}
      </div>
    </li>
  );
}, sameRow);
