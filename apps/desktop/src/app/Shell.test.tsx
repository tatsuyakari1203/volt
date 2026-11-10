import { act, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { ThemeProvider } from '../shared/theme';
import { ToastProvider } from '../shared/ui';
import { Shell } from './Shell';

// A plain browser has no Tauri bridge: the calls fail. The screens must absorb that. The tray
// Boost event is the exception: the test captures its handler to fire it by hand.
const { boostListeners, tweakReads } = vi.hoisted(() => ({
  boostListeners: [] as Array<() => void>,
  tweakReads: { count: 0 },
}));
vi.mock('../shared/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../shared/ipc')>()),
  getAppInfo: () => Promise.reject(new Error('no tauri bridge')),
  listTweaks: () => {
    tweakReads.count += 1;
    return Promise.reject(new Error('no tauri bridge'));
  },
  listActivity: () => Promise.reject(new Error('no tauri bridge')),
  getHealthReport: () => Promise.reject(new Error('no tauri bridge')),
  getLatestMetrics: () => Promise.reject(new Error('no tauri bridge')),
  onMetricsUpdated: () => Promise.reject(new Error('no tauri bridge')),
  getAutostartEnabled: () => Promise.reject(new Error('no tauri bridge')),
  setAutostartEnabled: () => Promise.reject(new Error('no tauri bridge')),
  onTrayBoostRequested: (handler: () => void) => {
    boostListeners.push(handler);
    return Promise.resolve(() => {
      boostListeners.splice(boostListeners.indexOf(handler), 1);
    });
  },
}));

function renderShell() {
  return render(
    <ThemeProvider>
      <ToastProvider>
        <Shell />
      </ToastProvider>
    </ThemeProvider>,
  );
}

it('lists every section in the primary navigation, with Home current', () => {
  renderShell();
  const nav = screen.getByRole('navigation', { name: 'Primary' });
  for (const label of [
    'Home',
    'Clean',
    'Memory',
    'Tweaks',
    'Apps and Startup',
    'Health',
    'Settings',
    'Undo',
  ]) {
    expect(within(nav).getByRole('button', { name: label })).toBeTruthy();
  }
  expect(within(nav).getByRole('button', { name: 'Home' }).getAttribute('aria-current')).toBe(
    'page',
  );
  expect(screen.getByRole('heading', { level: 1, name: 'Home' })).toBeTruthy();
});

it('navigates by keyboard and moves focus to the content', async () => {
  const user = userEvent.setup();
  renderShell();
  const nav = screen.getByRole('navigation', { name: 'Primary' });
  const tweaks = within(nav).getByRole('button', { name: 'Tweaks' });
  tweaks.focus();
  await user.keyboard('{Enter}');

  expect(tweaks.getAttribute('aria-current')).toBe('page');
  expect(screen.getByRole('heading', { level: 1, name: 'Tweaks' })).toBeTruthy();
  expect(document.activeElement).toBe(screen.getByRole('main'));
});

it('shows a placeholder that says what will live in an unbuilt section', async () => {
  const user = userEvent.setup();
  renderShell();
  await user.click(screen.getByRole('button', { name: 'Clean' }));
  expect(screen.getByRole('heading', { level: 1, name: 'Clean' })).toBeTruthy();
  expect(screen.getByText('Coming soon')).toBeTruthy();
  expect(screen.getByText(/Recycle Bin/)).toBeTruthy();
});

it('contains the Rust-core failure inside the About block in Settings', async () => {
  const user = userEvent.setup();
  renderShell();
  await user.click(screen.getByRole('button', { name: 'Settings' }));
  const about = screen.getByRole('region', { name: 'About' });
  expect(await within(about).findByText(/Could not reach the Rust core/)).toBeTruthy();
  // The rest of Settings and the shell are unaffected.
  expect(screen.getByRole('radiogroup', { name: 'Theme' })).toBeTruthy();
  expect(screen.getByRole('navigation', { name: 'Primary' })).toBeTruthy();
});

it('changing the theme in Settings applies and persists it', async () => {
  const user = userEvent.setup();
  renderShell();
  await user.click(screen.getByRole('button', { name: 'Settings' }));
  await user.click(screen.getByRole('radio', { name: 'Dark' }));
  expect(document.documentElement.dataset.theme).toBe('dark');
  expect(window.localStorage.getItem('volt.theme')).toBe('dark');
  await user.click(screen.getByRole('radio', { name: 'Light' }));
  expect(document.documentElement.dataset.theme).toBe('light');
});

it('shows the real Home, without the sample marker or a Boost button, and explains it cannot load here', async () => {
  renderShell();
  expect(screen.getByRole('heading', { level: 1, name: 'Home' })).toBeTruthy();
  expect(await screen.findByText('Your PC could not be checked')).toBeTruthy();
  expect(await screen.findByText('Live numbers are not available')).toBeTruthy();
  expect(screen.queryByText(/sample data/i)).toBeNull();
  expect(screen.queryByRole('button', { name: 'Boost' })).toBeNull();
});

it('the tray Boost event brings an open window back to Home', async () => {
  const user = userEvent.setup();
  renderShell();
  await user.click(screen.getByRole('button', { name: 'Tweaks' }));
  expect(screen.getByRole('heading', { level: 1, name: 'Tweaks' })).toBeTruthy();

  await vi.waitFor(() => expect(boostListeners).toHaveLength(1));
  act(() => {
    for (const listener of boostListeners) listener();
  });
  expect(screen.getByRole('heading', { level: 1, name: 'Home' })).toBeTruthy();
});

it('stops listening for the tray Boost event when the shell unmounts', async () => {
  const { unmount } = renderShell();
  await vi.waitFor(() => expect(boostListeners).toHaveLength(1));
  unmount();
  await vi.waitFor(() => expect(boostListeners).toHaveLength(0));
});

it('shows the Start with Windows switch in Settings, unavailable without the shell', async () => {
  const user = userEvent.setup();
  renderShell();
  await user.click(screen.getByRole('button', { name: 'Settings' }));
  const startup = screen.getByRole('region', { name: 'Startup' });
  const toggle = within(startup).getByRole('switch', { name: 'Start with Windows' });
  expect(toggle.hasAttribute('disabled')).toBe(true);
  expect(await within(startup).findByRole('alert')).toBeTruthy();
});

it('shows the real Tweaks screen without the sample marker, and explains that it cannot load here', async () => {
  const user = userEvent.setup();
  renderShell();
  await user.click(screen.getByRole('button', { name: 'Tweaks' }));
  expect(await screen.findByText('Tweaks could not be loaded')).toBeTruthy();
  expect(screen.queryByText('Sample data')).toBeNull();
});

it('shows the activity history on the Undo screen instead of a placeholder', async () => {
  const user = userEvent.setup();
  renderShell();
  await user.click(screen.getByRole('button', { name: 'Undo' }));
  expect(screen.getByRole('heading', { level: 1, name: 'Undo' })).toBeTruthy();
  expect(await screen.findByText('The history could not be loaded')).toBeTruthy();
  expect(screen.queryByText('Coming soon')).toBeNull();
});

it('reads the tweak states again when Tweaks is opened after the Undo screen', async () => {
  // Undoing a change on the Undo screen changes what Tweaks must show, and the screens are
  // mounted only while they are the current section, so opening Tweaks always reads live state.
  const user = userEvent.setup();
  renderShell();
  tweakReads.count = 0;
  await user.click(screen.getByRole('button', { name: 'Tweaks' }));
  await screen.findByText('Tweaks could not be loaded');
  expect(tweakReads.count).toBe(1);
  await user.click(screen.getByRole('button', { name: 'Undo' }));
  await screen.findByText('The history could not be loaded');
  await user.click(screen.getByRole('button', { name: 'Tweaks' }));
  await screen.findByText('Tweaks could not be loaded');
  expect(tweakReads.count).toBe(2);
});
