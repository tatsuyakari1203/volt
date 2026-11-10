import { getLocale, type MessageKey, t, tn } from '../i18n';
import type { AreaResult, MemoryAreaView, MemoryListsView, OperationState } from '../ipc';
import { formatBytes } from './size';

export { describeError } from './errors';

function assertNever(value: never): never {
  throw new Error(`Unhandled value: ${JSON.stringify(value)}`);
}

const MIB = 1024 * 1024;

/** The change between two readings, signed: "+1.2 GB", "-300 MB" or "no change". */
export function formatChange(before: number, after: number): string {
  const delta = after - before;
  // Under half a megabyte shows as 0 MB, so it is not a change anyone can see.
  if (Math.abs(delta) < MIB / 2) return t('memory.change.none');
  return `${delta > 0 ? '+' : '-'}${formatBytes(Math.abs(delta))}`;
}

/** The four lists in the order they are shown, with what each one is. */
export const listRows: ReadonlyArray<{
  key: keyof Omit<MemoryListsView, 'totalBytes'>;
  label: MessageKey;
  hint: MessageKey;
  swatch: string;
}> = [
  {
    key: 'inUseBytes',
    label: 'memory.list.inUse.label',
    hint: 'memory.list.inUse.hint',
    swatch: 'bg-accent',
  },
  {
    key: 'standbyBytes',
    label: 'memory.list.standby.label',
    hint: 'memory.list.standby.hint',
    swatch: 'bg-accent-soft',
  },
  {
    key: 'modifiedBytes',
    label: 'memory.list.modified.label',
    hint: 'memory.list.modified.hint',
    swatch: 'bg-caution',
  },
  {
    key: 'freeBytes',
    label: 'memory.list.free.label',
    hint: 'memory.list.free.hint',
    swatch: 'bg-surface-sunken',
  },
];

/** What each operation does and the one thing to know before running it. */
export function operationText(area: MemoryAreaView): { title: string; description: string } {
  switch (area) {
    case 'workingSets':
      return {
        title: t('memory.op.workingSets.title'),
        description: t('memory.op.workingSets.description'),
      };
    case 'fileCache':
      return {
        title: t('memory.op.fileCache.title'),
        description: t('memory.op.fileCache.description'),
      };
    case 'modifiedList':
      return {
        title: t('memory.op.modifiedList.title'),
        description: t('memory.op.modifiedList.description'),
      };
    case 'standbyListLowPriority':
      return {
        title: t('memory.op.standbyListLowPriority.title'),
        description: t('memory.op.standbyListLowPriority.description'),
      };
    case 'standbyList':
      return {
        title: t('memory.op.standbyList.title'),
        description: t('memory.op.standbyList.description'),
      };
    default:
      return assertNever(area);
  }
}

/**
 * Why an operation cannot run. The core writes the reason in English and that stays the source;
 * in another language the two reasons it has are told apart by the state and translated.
 */
export function reasonText(state: OperationState, reason: string | null): string | null {
  if (getLocale() === 'en') return reason;
  switch (state) {
    case 'available':
      return reason;
    case 'needsAdministrator':
      return t('memory.reason.needsAdministrator');
    case 'unsupported':
      return t('memory.reason.unsupported');
    default:
      return assertNever(state);
  }
}

/** One line saying how an operation of a run ended. */
export function describeArea(result: AreaResult): string {
  switch (result.status) {
    case 'done':
      if (result.area === 'workingSets') {
        const trimmed = tn('memory.area.trimmed', result.processesTrimmed);
        return result.processesSkipped > 0
          ? t('memory.area.trimmedWithSkipped', { trimmed, skipped: result.processesSkipped })
          : t('memory.area.trimmedOnly', { trimmed });
      }
      return t('memory.area.done');
    case 'needsAdministrator':
      return t('memory.area.needsAdministrator');
    case 'unsupported':
      return t('memory.area.unsupported');
    case 'failed':
      return result.detail
        ? t('memory.area.failedWithDetail', { detail: result.detail })
        : t('memory.area.failed');
    default:
      return assertNever(result.status);
  }
}
