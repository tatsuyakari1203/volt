import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';
import type { ActivityItem, ActivityPage } from '../../shared/ipc';
import { IpcUnavailableError } from '../../shared/ipc';
import { UndoScreen } from './UndoScreen';

const ipc = vi.hoisted(() => ({ listActivity: vi.fn() }));
vi.mock('../../shared/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../shared/ipc')>()),
  ...ipc,
}));

function item(id: string, status: ActivityItem['status'], summary: string): ActivityItem {
  return {
    id,
    time: '2026-11-10T03:01:00.123Z',
    source: 'manual',
    status,
    summary,
    label: 'show-file-extensions',
  };
}

function page(items: ActivityItem[], nextCursor: string | null = null): ActivityPage {
  return { items, nextCursor };
}

beforeEach(() => {
  ipc.listActivity.mockReset();
});

it('shows a loading state, then the records newest first with time, outcome and summary', async () => {
  ipc.listActivity.mockResolvedValue(
    page([
      item('2', 'reverted', 'Reverted registry value HKCU\\X\\Flag, restored (not set)'),
      item('1', 'applied', 'Applied registry value HKCU\\X\\Flag = REG_DWORD 0'),
    ]),
  );
  render(<UndoScreen />);
  expect(screen.getByRole('status').textContent).toContain('Reading the history');

  const list = await screen.findByRole('list', { name: 'Recent activity' });
  const rows = within(list).getAllByRole('listitem');
  expect(rows).toHaveLength(2);
  // Newest first: the order the shell sent is the order shown.
  expect(rows[0]?.textContent).toContain('Undone');
  expect(rows[0]?.textContent).toContain('restored (not set)');
  expect(rows[1]?.textContent).toContain('Applied');
  expect(rows[1]?.textContent).toContain('REG_DWORD 0');
  const time = rows[0]?.querySelector('time');
  expect(time?.getAttribute('datetime')).toBe('2026-11-10T03:01:00.123Z');
  expect(time?.textContent).not.toBe('');
  expect(ipc.listActivity).toHaveBeenCalledWith(50);
});

it('has a plain empty state', async () => {
  ipc.listActivity.mockResolvedValue(page([]));
  render(<UndoScreen />);
  expect(await screen.findByText('Nothing yet')).toBeTruthy();
  expect(screen.queryByRole('list')).toBeNull();
});

it('explains a failed read and reads again on Try again', async () => {
  const user = userEvent.setup();
  ipc.listActivity.mockRejectedValueOnce(new Error('database is locked'));
  render(<UndoScreen />);
  const alert = await screen.findByRole('alert');
  expect(alert.textContent).toContain('The history could not be loaded');
  expect(alert.textContent).toContain('database is locked');

  ipc.listActivity.mockResolvedValue(page([item('1', 'applied', 'Applied it')]));
  await user.click(screen.getByRole('button', { name: 'Try again' }));
  expect(await screen.findByRole('list', { name: 'Recent activity' })).toBeTruthy();
});

it('says the history needs the Volt app when run in a plain browser', async () => {
  ipc.listActivity.mockRejectedValue(new IpcUnavailableError());
  render(<UndoScreen />);
  const alert = await screen.findByRole('alert');
  expect(alert.textContent).toContain('only available inside the Volt app');
});

it('loads older entries with the cursor and appends them', async () => {
  const user = userEvent.setup();
  ipc.listActivity.mockResolvedValueOnce(page([item('3', 'applied', 'Third')], '3'));
  render(<UndoScreen />);
  const more = await screen.findByRole('button', { name: 'Show older' });

  ipc.listActivity.mockResolvedValueOnce(page([item('2', 'failed', 'Second')]));
  await user.click(more);
  expect(ipc.listActivity).toHaveBeenLastCalledWith(50, '3');
  await vi.waitFor(() =>
    expect(within(screen.getByRole('list')).getAllByRole('listitem')).toHaveLength(2),
  );
  expect(screen.queryByRole('button', { name: 'Show older' })).toBeNull();
});
