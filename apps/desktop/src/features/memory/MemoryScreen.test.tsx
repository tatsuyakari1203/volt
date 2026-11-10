import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { MemoryCleanResult, MemoryListsView, MemoryState } from '../../shared/ipc';
import { IpcCommandError, IpcUnavailableError } from '../../shared/ipc';
import { describeArea, formatBytes, formatChange } from '../../shared/text/memory';
import { MemoryScreen } from './MemoryScreen';

const ipc = vi.hoisted(() => ({
  getMemoryState: vi.fn(),
  cleanMemory: vi.fn(),
}));
vi.mock('../../shared/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../shared/ipc')>()),
  ...ipc,
}));

const GB = 1024 ** 3;

const lists: MemoryListsView = {
  totalBytes: 16 * GB,
  inUseBytes: 8 * GB,
  standbyBytes: 4 * GB,
  modifiedBytes: 1 * GB,
  freeBytes: 3 * GB,
};

const state: MemoryState = {
  lists,
  problem: null,
  operations: [
    { area: 'workingSets', state: 'available', reason: null },
    { area: 'fileCache', state: 'needsAdministrator', reason: 'Needs administrator rights.' },
    { area: 'modifiedList', state: 'needsAdministrator', reason: 'Needs administrator rights.' },
    {
      area: 'standbyListLowPriority',
      state: 'needsAdministrator',
      reason: 'Needs administrator rights.',
    },
    { area: 'standbyList', state: 'unsupported', reason: 'Not on this Windows.' },
  ],
};

const report: MemoryCleanResult = {
  before: lists,
  after: { ...lists, inUseBytes: 7 * GB, standbyBytes: 5 * GB },
  areas: [
    {
      area: 'workingSets',
      status: 'done',
      processesTrimmed: 41,
      processesSkipped: 3,
      detail: null,
    },
  ],
  recorded: true,
};

beforeEach(() => {
  ipc.getMemoryState.mockReset();
  ipc.cleanMemory.mockReset();
});

describe('MemoryScreen', () => {
  it('shows memory by list and one button per operation, with the reason when one cannot run', async () => {
    ipc.getMemoryState.mockResolvedValue(state);
    render(<MemoryScreen />);

    const card = await screen.findByRole('region', { name: 'Memory by list' });
    for (const label of ['In use', 'Standby', 'Modified', 'Free']) {
      expect(within(card).getByText(label)).toBeTruthy();
    }
    expect(within(card).getByText('16 GB')).toBeTruthy();

    const operations = screen.getByRole('list', { name: 'Cleaning operations' });
    expect(within(operations).getAllByRole('button')).toHaveLength(5);
    expect(
      (screen.getByRole('button', { name: 'Run: Trim working sets' }) as HTMLButtonElement)
        .disabled,
    ).toBe(false);
    const standby = screen.getByRole('button', { name: 'Run: Purge the standby list' });
    expect((standby as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByText('Not on this Windows.')).toBeTruthy();
    expect(screen.getAllByText('Needs administrator rights.').length).toBeGreaterThan(0);
  });

  it('says when cleaning helps and when it does not', async () => {
    ipc.getMemoryState.mockResolvedValue(state);
    render(<MemoryScreen />);
    expect(await screen.findByText(/It does not help/)).toBeTruthy();
    expect(screen.getByText(/Windows gives it back by itself/)).toBeTruthy();
  });

  it('runs one operation and shows the before and after', async () => {
    const user = userEvent.setup();
    ipc.getMemoryState.mockResolvedValue(state);
    ipc.cleanMemory.mockResolvedValue(report);
    render(<MemoryScreen />);

    await user.click(await screen.findByRole('button', { name: 'Run: Trim working sets' }));

    expect(ipc.cleanMemory).toHaveBeenCalledWith(['workingSets']);
    const result = await screen.findByRole('region', { name: 'Result of the last run' });
    expect(within(result).getByText('-1 GB')).toBeTruthy();
    expect(within(result).getByText('+1 GB')).toBeTruthy();
    expect(within(result).getByText(/41 programs trimmed, 3 could not be opened/)).toBeTruthy();
    expect(within(result).getByText(/cannot be undone/)).toBeTruthy();
    // The state is read again so the numbers on the page are true now.
    await waitFor(() => expect(ipc.getMemoryState).toHaveBeenCalledTimes(2));
  });

  it('disables every button while an operation runs', async () => {
    const user = userEvent.setup();
    ipc.getMemoryState.mockResolvedValue({
      ...state,
      operations: state.operations.map((op) => ({ ...op, state: 'available', reason: null })),
    });
    let finish: (value: MemoryCleanResult) => void = () => {};
    ipc.cleanMemory.mockReturnValue(
      new Promise<MemoryCleanResult>((resolve) => {
        finish = resolve;
      }),
    );
    render(<MemoryScreen />);
    await user.click(await screen.findByRole('button', { name: 'Run: Purge the standby list' }));

    const other = screen.getByRole('button', { name: 'Run: Trim working sets' });
    expect((other as HTMLButtonElement).disabled).toBe(true);
    finish(report);
    await waitFor(() => expect((other as HTMLButtonElement).disabled).toBe(false));
  });

  it('shows a failed command as a notice and keeps the page usable', async () => {
    const user = userEvent.setup();
    ipc.getMemoryState.mockResolvedValue(state);
    ipc.cleanMemory.mockRejectedValue(
      new IpcCommandError({ kind: 'other', message: 'the background task failed' }),
    );
    render(<MemoryScreen />);
    await user.click(await screen.findByRole('button', { name: 'Run: Trim working sets' }));
    expect((await screen.findByRole('alert')).textContent).toContain('the background task failed');
    expect(screen.getByRole('region', { name: 'Memory by list' })).toBeTruthy();
  });

  it('explains that it cannot load outside the app, and can try again', async () => {
    const user = userEvent.setup();
    ipc.getMemoryState.mockRejectedValueOnce(new IpcUnavailableError());
    render(<MemoryScreen />);
    expect(await screen.findByText('Memory could not be read')).toBeTruthy();
    expect(screen.getByText('This is only available inside the Volt app.')).toBeTruthy();

    ipc.getMemoryState.mockResolvedValue(state);
    await user.click(screen.getByRole('button', { name: 'Try again' }));
    expect(await screen.findByRole('region', { name: 'Memory by list' })).toBeTruthy();
  });

  it('still lists the operations when memory by list cannot be read', async () => {
    ipc.getMemoryState.mockResolvedValue({ ...state, lists: null, problem: 'boom' });
    render(<MemoryScreen />);
    expect(await screen.findByText(/Memory by list could not be read: boom/)).toBeTruthy();
    expect(screen.getByRole('list', { name: 'Cleaning operations' })).toBeTruthy();
  });
});

describe('memory messages', () => {
  it('formats sizes and changes', () => {
    expect(formatBytes(4 * GB)).toBe('4 GB');
    expect(formatBytes(512 * 1024 ** 2)).toBe('512 MB');
    expect(formatChange(4 * GB, 5 * GB)).toBe('+1 GB');
    expect(formatChange(5 * GB, 4.5 * GB)).toBe('-512 MB');
    expect(formatChange(GB, GB + 1000)).toBe('no change');
  });

  it('describes each way an operation can end', () => {
    const base = {
      area: 'standbyList',
      processesTrimmed: 0,
      processesSkipped: 0,
      detail: null,
    } as const;
    expect(describeArea({ ...base, status: 'needsAdministrator' })).toMatch(/administrator/);
    expect(describeArea({ ...base, status: 'unsupported' })).toMatch(/does not support/);
    expect(describeArea({ ...base, status: 'failed', detail: 'x' })).toBe('Failed: x');
    expect(describeArea({ ...base, status: 'done' })).toBe('Done.');
  });
});
