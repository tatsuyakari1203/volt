import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';
import { StartupSettings } from './StartupSettings';

const { getAutostartEnabled, setAutostartEnabled } = vi.hoisted(() => ({
  getAutostartEnabled: vi.fn(),
  setAutostartEnabled: vi.fn(),
}));
vi.mock('../../shared/ipc', () => ({ getAutostartEnabled, setAutostartEnabled }));

beforeEach(() => {
  getAutostartEnabled.mockReset();
  setAutostartEnabled.mockReset();
});

it('shows the switch off when the shell reports start with Windows is off', async () => {
  getAutostartEnabled.mockResolvedValue(false);
  render(<StartupSettings />);
  const toggle = await screen.findByRole('switch', { name: 'Start with Windows' });
  await vi.waitFor(() => expect(toggle.hasAttribute('disabled')).toBe(false));
  expect(toggle.getAttribute('aria-checked')).toBe('false');
  expect(setAutostartEnabled).not.toHaveBeenCalled();
});

it('is unavailable until the state has been read', () => {
  getAutostartEnabled.mockReturnValue(new Promise(() => {}));
  render(<StartupSettings />);
  expect(screen.getByRole('switch', { name: 'Start with Windows' }).hasAttribute('disabled')).toBe(
    true,
  );
});

it('turns it on through the typed command and shows the state the shell reports', async () => {
  const user = userEvent.setup();
  getAutostartEnabled.mockResolvedValue(false);
  setAutostartEnabled.mockResolvedValue(true);
  render(<StartupSettings />);
  const toggle = await screen.findByRole('switch', { name: 'Start with Windows' });
  await vi.waitFor(() => expect(toggle.hasAttribute('disabled')).toBe(false));

  await user.click(toggle);
  expect(setAutostartEnabled).toHaveBeenCalledTimes(1);
  expect(setAutostartEnabled).toHaveBeenCalledWith(true);
  await vi.waitFor(() => expect(toggle.getAttribute('aria-checked')).toBe('true'));
});

it('turns it back off', async () => {
  const user = userEvent.setup();
  getAutostartEnabled.mockResolvedValue(true);
  setAutostartEnabled.mockResolvedValue(false);
  render(<StartupSettings />);
  const toggle = await screen.findByRole('switch', { name: 'Start with Windows' });
  await vi.waitFor(() => expect(toggle.getAttribute('aria-checked')).toBe('true'));

  await user.click(toggle);
  expect(setAutostartEnabled).toHaveBeenCalledWith(false);
  await vi.waitFor(() => expect(toggle.getAttribute('aria-checked')).toBe('false'));
});

it('keeps the old state and says so when the shell refuses the change', async () => {
  const user = userEvent.setup();
  getAutostartEnabled.mockResolvedValue(false);
  setAutostartEnabled.mockRejectedValue(new Error('registry unavailable'));
  render(<StartupSettings />);
  const toggle = await screen.findByRole('switch', { name: 'Start with Windows' });
  await vi.waitFor(() => expect(toggle.hasAttribute('disabled')).toBe(false));

  await user.click(toggle);
  const alert = await screen.findByRole('alert');
  expect(alert.textContent).toContain('registry unavailable');
  expect(toggle.getAttribute('aria-checked')).toBe('false');
  await vi.waitFor(() => expect(toggle.hasAttribute('disabled')).toBe(false));
});

it('reports a failure to read the state and leaves the switch unavailable', async () => {
  getAutostartEnabled.mockRejectedValue(new Error('no tauri bridge'));
  render(<StartupSettings />);
  const alert = await screen.findByRole('alert');
  expect(alert.textContent).toContain('no tauri bridge');
  expect(screen.getByRole('switch', { name: 'Start with Windows' }).hasAttribute('disabled')).toBe(
    true,
  );
});
