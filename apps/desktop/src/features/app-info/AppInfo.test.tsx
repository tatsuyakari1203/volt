import { render, screen } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { IpcUnavailableError } from '../../shared/ipc';
import { AppInfo } from './AppInfo';

const ipc = vi.hoisted(() => ({ getAppInfo: vi.fn() }));
vi.mock('../../shared/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../shared/ipc')>()),
  ...ipc,
}));

beforeEach(() => {
  ipc.getAppInfo.mockReset();
});

it('shows the app name as a secondary heading, with the version returned by the Rust core', async () => {
  ipc.getAppInfo.mockResolvedValue({ name: 'Volt', version: '1.2.3' });
  render(<AppInfo />);
  // A card-sized secondary heading, not a second page title.
  expect(await screen.findByRole('heading', { level: 3, name: 'Volt' })).toBeTruthy();
  expect(screen.queryByRole('heading', { level: 1 })).toBeNull();
  expect(screen.getByText('Version 1.2.3')).toBeTruthy();
});

it('says it needs the Volt app, instead of a raw error, when run in a plain browser', async () => {
  ipc.getAppInfo.mockRejectedValue(new IpcUnavailableError());
  render(<AppInfo />);
  expect(
    await screen.findByText('Version information is not available outside the Volt app.'),
  ).toBeTruthy();
  expect(screen.queryByText(/TypeError/)).toBeNull();
  expect(screen.queryByText(/Could not reach/)).toBeNull();
});

it('still reports any other failure with its reason', async () => {
  ipc.getAppInfo.mockRejectedValue(new Error('the core is down'));
  render(<AppInfo />);
  expect(await screen.findByText(/Could not reach the Rust core: .*the core is down/)).toBeTruthy();
});
