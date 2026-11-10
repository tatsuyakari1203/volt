import {
  type AreaResult,
  describeIpcError,
  IpcCommandError,
  IpcUnavailableError,
  type MemoryAreaView,
  type MemoryListsView,
} from '../ipc';

function assertNever(value: never): never {
  throw new Error(`Unhandled value: ${JSON.stringify(value)}`);
}

/** One sentence for any error that reached the UI. */
export function describeError(error: unknown): string {
  if (error instanceof IpcUnavailableError) {
    return 'This is only available inside the Volt app.';
  }
  if (error instanceof IpcCommandError) {
    return describeIpcError(error.error);
  }
  return error instanceof Error ? error.message : String(error);
}

const KIB = 1024;
const MIB = KIB * 1024;
const GIB = MIB * 1024;

/** A byte count the way Windows writes it: binary units labelled GB and MB. */
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return '—';
  if (bytes >= GIB) {
    const gib = bytes / GIB;
    return `${gib >= 100 ? Math.round(gib) : gib.toFixed(1).replace(/\.0$/, '')} GB`;
  }
  return `${Math.round(bytes / MIB)} MB`;
}

/** The change between two readings, signed: "+1.2 GB", "-300 MB" or "no change". */
export function formatChange(before: number, after: number): string {
  const delta = after - before;
  // Under half a megabyte shows as 0 MB, so it is not a change anyone can see.
  if (Math.abs(delta) < MIB / 2) return 'no change';
  return `${delta > 0 ? '+' : '-'}${formatBytes(Math.abs(delta))}`;
}

/** The four lists in the order they are shown, with what each one is. */
export const listRows: ReadonlyArray<{
  key: keyof Omit<MemoryListsView, 'totalBytes'>;
  label: string;
  hint: string;
  swatch: string;
}> = [
  {
    key: 'inUseBytes',
    label: 'In use',
    hint: 'Held by programs, drivers and Windows itself.',
    swatch: 'bg-accent',
  },
  {
    key: 'standbyBytes',
    label: 'Standby',
    hint: 'A cache of data Windows can reload. It gives this back by itself when memory is needed.',
    swatch: 'bg-accent-soft',
  },
  {
    key: 'modifiedBytes',
    label: 'Modified',
    hint: 'Changed data waiting to be written to disk.',
    swatch: 'bg-caution',
  },
  {
    key: 'freeBytes',
    label: 'Free',
    hint: 'Memory nobody is using.',
    swatch: 'bg-surface-sunken',
  },
];

/** What each operation does and the one thing to know before running it. */
export const operationText: Record<MemoryAreaView, { title: string; description: string }> = {
  workingSets: {
    title: 'Trim working sets',
    description:
      'Asks every program you can open to hand back memory it is not using right now. Programs reload what they need, so they may pause briefly.',
  },
  fileCache: {
    title: 'Trim the file cache',
    description: 'Releases copies of files that Windows keeps in memory for faster reading.',
  },
  modifiedList: {
    title: 'Flush the modified list',
    description:
      'Writes changed data to disk so the memory can be reused. This writes to disk and can take a moment.',
  },
  standbyListLowPriority: {
    title: 'Purge the low-priority standby list',
    description: 'Drops the cached data Windows would give up first (priority 0).',
  },
  standbyList: {
    title: 'Purge the standby list',
    description:
      'Drops all cached data. What you open next loads from disk again, so it can feel slower for a while.',
  },
};

/** One line saying how an operation of a run ended. */
export function describeArea(result: AreaResult): string {
  switch (result.status) {
    case 'done':
      if (result.area === 'workingSets') {
        const trimmed = `${result.processesTrimmed} ${result.processesTrimmed === 1 ? 'program' : 'programs'} trimmed`;
        return result.processesSkipped > 0
          ? `${trimmed}, ${result.processesSkipped} could not be opened and were left alone.`
          : `${trimmed}.`;
      }
      return 'Done.';
    case 'needsAdministrator':
      return 'Needs administrator rights, so nothing was changed.';
    case 'unsupported':
      return 'This version of Windows does not support it, so nothing was changed.';
    case 'failed':
      return result.detail ? `Failed: ${result.detail}` : 'Failed.';
    default:
      return assertNever(result.status);
  }
}
