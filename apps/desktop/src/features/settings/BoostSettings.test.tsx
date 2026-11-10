import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';
import type { BoostConfigView, MemoryState } from '../../shared/ipc';
import { BoostSettings } from './BoostSettings';

const ipc = vi.hoisted(() => ({
  getBoostConfig: vi.fn(),
  setBoostConfig: vi.fn(),
  getMemoryState: vi.fn(),
}));
vi.mock('../../shared/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../shared/ipc')>()),
  ...ipc,
}));

const memory: MemoryState = {
  lists: null,
  problem: null,
  operations: [
    { area: 'workingSets', state: 'available', reason: null },
    { area: 'standbyList', state: 'needsAdministrator', reason: 'Needs administrator rights.' },
  ],
};

beforeEach(() => {
  for (const mock of Object.values(ipc)) mock.mockReset();
  ipc.getMemoryState.mockResolvedValue(memory);
});

it('shows the defaults: working sets on, the administrator operation off and unavailable, no cleaning', async () => {
  ipc.getBoostConfig.mockResolvedValue({ areas: ['workingSets'], moveTempToRecycleBin: false });
  render(<BoostSettings />);
  const workingSets = await screen.findByRole('switch', { name: 'Trim working sets' });
  await vi.waitFor(() => expect(workingSets.hasAttribute('disabled')).toBe(false));
  expect(workingSets.getAttribute('aria-checked')).toBe('true');
  const standby = screen.getByRole('switch', { name: 'Purge the standby list' });
  expect(standby.hasAttribute('disabled')).toBe(true);
  expect(standby.getAttribute('aria-checked')).toBe('false');
  expect(screen.getByText('Needs administrator rights')).toBeTruthy();
  const temp = screen.getByRole('switch', { name: 'Also move temp files to the Recycle Bin' });
  expect(temp.getAttribute('aria-checked')).toBe('false');
  expect(screen.getByText(/skips the preview/)).toBeTruthy();
});

it('saves the opt-in through the typed command and shows what the shell stored', async () => {
  const user = userEvent.setup();
  const stored: BoostConfigView = { areas: ['workingSets'], moveTempToRecycleBin: true };
  ipc.getBoostConfig.mockResolvedValue({ areas: ['workingSets'], moveTempToRecycleBin: false });
  ipc.setBoostConfig.mockResolvedValue(stored);
  render(<BoostSettings />);
  const temp = await screen.findByRole('switch', {
    name: 'Also move temp files to the Recycle Bin',
  });
  await vi.waitFor(() => expect(temp.hasAttribute('disabled')).toBe(false));
  await user.click(temp);
  expect(ipc.setBoostConfig).toHaveBeenCalledWith(stored);
  await vi.waitFor(() => expect(temp.getAttribute('aria-checked')).toBe('true'));
});

it('removes an operation from Boost when it is switched off', async () => {
  const user = userEvent.setup();
  ipc.getBoostConfig.mockResolvedValue({ areas: ['workingSets'], moveTempToRecycleBin: false });
  ipc.setBoostConfig.mockResolvedValue({ areas: [], moveTempToRecycleBin: false });
  render(<BoostSettings />);
  const workingSets = await screen.findByRole('switch', { name: 'Trim working sets' });
  await vi.waitFor(() => expect(workingSets.hasAttribute('disabled')).toBe(false));
  await user.click(workingSets);
  expect(ipc.setBoostConfig).toHaveBeenCalledWith({ areas: [], moveTempToRecycleBin: false });
});

it('says so when the settings cannot be read', async () => {
  ipc.getBoostConfig.mockRejectedValue(new Error('no bridge'));
  render(<BoostSettings />);
  expect((await screen.findByRole('alert')).textContent).toContain('Could not read');
});
