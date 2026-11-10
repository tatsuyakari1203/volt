import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';
import { setLocale } from '../../shared/i18n';
import type { TweakInfo } from '../../shared/ipc';
import { ToastProvider } from '../../shared/ui';
import { TweaksScreen } from './TweaksScreen';

const ipc = vi.hoisted(() => ({
  listTweaks: vi.fn(),
  applyTweak: vi.fn(),
  revertTweak: vi.fn(),
  getTweakPlan: vi.fn(),
  resetTweak: vi.fn(),
}));
vi.mock('../../shared/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../shared/ipc')>()),
  ...ipc,
}));

const fileExtensions: TweakInfo = {
  id: 'show-file-extensions',
  title: 'Show file extensions',
  description: 'Always show file endings such as .exe and .pdf in File Explorer.',
  category: 'appearance',
  risk: 'safe',
  evidence: 'anecdotal',
  evidenceNote: 'A usability and safety preference, not a performance gain.',
  restart: 'signOut',
  needsAdmin: false,
  state: { kind: 'notApplied' },
  appliedByVolt: false,
};

// A tweak a later version adds: it has no Vietnamese text yet and must still be shown.
const newer: TweakInfo = {
  ...fileExtensions,
  id: 'added-later',
  title: 'A tweak added later',
  description: 'The core wrote this in English.',
};

beforeEach(() => {
  Object.values(ipc).forEach((mock) => {
    mock.mockReset();
  });
  setLocale('vi');
});

it('shows the title and description of a tweak in Vietnamese, filed under its id', async () => {
  ipc.listTweaks.mockResolvedValue([fileExtensions]);
  render(
    <ToastProvider>
      <TweaksScreen />
    </ToastProvider>,
  );
  expect(await screen.findByText('Hiện đuôi tệp')).toBeTruthy();
  expect(screen.getByText(/Luôn hiện phần đuôi tệp/)).toBeTruthy();
  expect(screen.getByRole('switch', { name: 'Hiện đuôi tệp' })).toBeTruthy();
  expect(screen.getByRole('heading', { level: 1, name: 'Tinh chỉnh' })).toBeTruthy();
  expect(screen.queryByText('Show file extensions')).toBeNull();
});

it('writes the result of turning a tweak on in Vietnamese, with its restart note', async () => {
  const user = userEvent.setup();
  ipc.listTweaks.mockResolvedValue([fileExtensions]);
  ipc.applyTweak.mockResolvedValue({
    tweak: fileExtensions.id,
    status: 'applied',
    detail: null,
    state: { kind: 'applied' },
  });
  render(
    <ToastProvider>
      <TweaksScreen />
    </ToastProvider>,
  );
  await user.click(await screen.findByRole('switch', { name: 'Hiện đuôi tệp' }));
  expect(
    await screen.findByText('"Hiện đuôi tệp" đã bật. Hãy đăng xuất rồi đăng nhập lại để hoàn tất.'),
  ).toBeTruthy();
});

it('falls back to the core English for a tweak with no Vietnamese text', async () => {
  ipc.listTweaks.mockResolvedValue([newer]);
  render(
    <ToastProvider>
      <TweaksScreen />
    </ToastProvider>,
  );
  expect(await screen.findByText('A tweak added later')).toBeTruthy();
  expect(screen.getByText('The core wrote this in English.')).toBeTruthy();
});
