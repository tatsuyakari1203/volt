import { render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { AppInfo } from './AppInfo';

vi.mock('../../shared/ipc', () => ({
  getAppInfo: () => Promise.resolve({ name: 'Volt', version: '1.2.3' }),
}));

it('shows the app name and version returned by the Rust core', async () => {
  render(<AppInfo />);
  expect(await screen.findByRole('heading', { name: 'Volt' })).toBeTruthy();
  expect(screen.getByText('Version 1.2.3')).toBeTruthy();
});
