import { act, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { I18nProvider } from '../shared/i18n';
import { ThemeProvider } from '../shared/theme';
import { ToastProvider } from '../shared/ui';
import { Shell } from './Shell';

// A plain browser has no Tauri bridge: the calls fail. The screens must absorb that. The tray
// Boost event is the exception: the test captures its handler to fire it by hand.
const { boostListeners, tweakReads, saveLanguage, languageSetting } = vi.hoisted(() => ({
  boostListeners: [] as Array<(report: unknown) => void>,
  tweakReads: { count: 0 },
  saveLanguage: vi.fn(() => Promise.resolve({ choice: 'vi', system: 'en' })),
  languageSetting: vi.fn<() => Promise<{ choice: string; system: string }>>(() =>
    Promise.reject(new Error('no tauri bridge')),
  ),
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
  listStartupEntries: () => Promise.reject(new Error('no tauri bridge')),
  listApps: () => Promise.reject(new Error('no tauri bridge')),
  getAutostartEnabled: () => Promise.reject(new Error('no tauri bridge')),
  getLanguageSetting: languageSetting,
  saveLanguageSetting: saveLanguage,
  setAutostartEnabled: () => Promise.reject(new Error('no tauri bridge')),
  onBoostFinished: (handler: (report: unknown) => void) => {
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
    'Startup',
    'Apps',
    'Processes',
    'Settings',
    'Undo',
  ]) {
    expect(within(nav).getByRole('button', { name: label })).toBeTruthy();
  }
  // Nothing is listed for a section that is not built.
  expect(within(nav).queryByRole('button', { name: 'Health' })).toBeNull();
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

it('shows the real Home, without the sample marker, and explains it cannot load here', async () => {
  renderShell();
  expect(screen.getByRole('heading', { level: 1, name: 'Home' })).toBeTruthy();
  expect(await screen.findByText('Your PC could not be checked')).toBeTruthy();
  expect(await screen.findByText('Live numbers are not available')).toBeTruthy();
  expect(screen.queryByText(/sample data/i)).toBeNull();
  expect(screen.getByRole('button', { name: 'Boost' })).toBeTruthy();
});

it('the tray Boost report brings an open window back to Home and shows it', async () => {
  const user = userEvent.setup();
  renderShell();
  await user.click(screen.getByRole('button', { name: 'Tweaks' }));
  expect(screen.getByRole('heading', { level: 1, name: 'Tweaks' })).toBeTruthy();

  await vi.waitFor(() => expect(boostListeners).toHaveLength(1));
  act(() => {
    for (const listener of boostListeners) {
      listener({
        memoryBefore: null,
        memoryAfter: null,
        availableBefore: 4 * 1024 ** 3,
        availableAfter: 5 * 1024 ** 3,
        areas: [],
        skipped: [],
        junk: null,
        movedToBin: null,
        recorded: true,
      });
    }
  });
  expect(screen.getByRole('heading', { level: 1, name: 'Home' })).toBeTruthy();
  expect(screen.getByRole('region', { name: 'Boost report' })).toBeTruthy();
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

it('shows the real Startup screen, and explains that it cannot load here', async () => {
  const user = userEvent.setup();
  renderShell();
  await user.click(screen.getByRole('button', { name: 'Startup' }));
  expect(screen.getByRole('heading', { level: 1, name: 'Startup' })).toBeTruthy();
  expect(await screen.findByText('Startup entries could not be loaded')).toBeTruthy();
});

it('shows the real Memory screen, and explains that it cannot load here', async () => {
  const user = userEvent.setup();
  renderShell();
  await user.click(screen.getByRole('button', { name: 'Memory' }));
  expect(screen.getByRole('heading', { level: 1, name: 'Memory' })).toBeTruthy();
  expect(await screen.findByText('Memory could not be read')).toBeTruthy();
});

it('shows the activity history on the Undo screen instead of a placeholder', async () => {
  const user = userEvent.setup();
  renderShell();
  await user.click(screen.getByRole('button', { name: 'Undo' }));
  expect(screen.getByRole('heading', { level: 1, name: 'Undo' })).toBeTruthy();
  expect(await screen.findByText('The history could not be loaded')).toBeTruthy();
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

function renderLocalisedShell() {
  return render(
    <I18nProvider>
      <ThemeProvider>
        <ToastProvider>
          <Shell />
        </ToastProvider>
      </ThemeProvider>
    </I18nProvider>,
  );
}

it('choosing Tiếng Việt in Settings translates the window at once and saves the choice', async () => {
  const user = userEvent.setup();
  renderLocalisedShell();
  await user.click(screen.getByRole('button', { name: 'Settings' }));
  await user.click(screen.getByRole('radio', { name: 'Tiếng Việt' }));

  // The page stays where it was and is written in Vietnamese; nothing needed a restart.
  expect(screen.getByRole('heading', { level: 1, name: 'Cài đặt' })).toBeTruthy();
  expect(screen.getByRole('navigation', { name: 'Điều hướng chính' })).toBeTruthy();
  expect(screen.getByRole('button', { name: 'Hoàn tác' })).toBeTruthy();
  expect(document.documentElement.lang).toBe('vi');
  expect(window.localStorage.getItem('volt.language')).toBe('vi');
  // The shell learns the choice and what System means on this PC (English here).
  expect(saveLanguage).toHaveBeenCalledWith('vi');

  await user.click(screen.getByRole('radio', { name: 'English' }));
  expect(screen.getByRole('heading', { level: 1, name: 'Settings' })).toBeTruthy();
  expect(window.localStorage.getItem('volt.language')).toBe('en');
});

it('follows the display language of Windows, as the shell reports it, by default', async () => {
  languageSetting.mockResolvedValueOnce({ choice: 'system', system: 'vi' });
  renderLocalisedShell();
  expect(
    await screen.findByRole('heading', { level: 1, name: 'Trang chủ' }, { timeout: 2000 }),
  ).toBeTruthy();
  expect(window.localStorage.getItem('volt.language.system')).toBe('vi');
});

it('never takes the language from the browser language list', () => {
  const languages = vi.spyOn(window.navigator, 'languages', 'get').mockReturnValue(['vi-VN']);
  try {
    renderLocalisedShell();
    expect(screen.getByRole('heading', { level: 1, name: 'Home' })).toBeTruthy();
  } finally {
    languages.mockRestore();
  }
});
