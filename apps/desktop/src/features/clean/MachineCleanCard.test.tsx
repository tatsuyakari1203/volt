import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { setLocale } from '../../shared/i18n';
import type {
  CleaningRuleInfo,
  MachineCleanResult,
  MachineScanView,
  PhraseView,
} from '../../shared/ipc';
import { MachineCleanCard } from './MachineCleanCard';

const ipc = vi.hoisted(() => ({
  getElevationStatus: vi.fn(),
  scanMachineRule: vi.fn(),
  cleanMachineRule: vi.fn(),
}));
vi.mock('../../shared/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../shared/ipc')>()),
  ...ipc,
}));

function rule(id: string, title: string): CleaningRuleInfo {
  return {
    id,
    title,
    description: 'Left by Windows for all users.',
    category: 'temporaryFiles',
    categoryLabel: 'Temporary files',
    risk: 'caution',
    needsAdministrator: true,
    minAgeDays: 7,
    pickItems: false,
    needsFolder: false,
    chosenFolder: null,
    follows: [],
  };
}

const temp = rule('windows-temp', 'Windows temporary files');
const logs = rule('windows-update-logs', 'Windows Update trace logs');

function scanView(id: string, extra: Partial<MachineScanView> = {}): MachineScanView {
  return {
    ruleId: id,
    files: 10,
    folders: 2,
    bytes: 3 * 1024 ** 2,
    minAgeDays: 7,
    truncated: false,
    confirm: false,
    canClean: true,
    skipped: null,
    skippedPhrase: null,
    problem: null,
    problemPhrase: null,
    ...extra,
  };
}

function phrase(code: string): PhraseView {
  return { code, args: {} };
}

function done(extra: Partial<Extract<MachineCleanResult, { outcome: 'done' }>> = {}) {
  return {
    outcome: 'done' as const,
    ruleId: 'windows-temp',
    files: 9,
    folders: 2,
    bytes: 3000000,
    skippedTotal: 0,
    skipped: null,
    skippedPhrase: null,
    stopped: false,
    message:
      'Permanently deleted 9 files (2.9 MB) for rule windows-temp. They did not go to the Recycle Bin and this cannot be undone',
    messagePhrase: phrase('summary.cleanMachine'),
    historyIncomplete: false,
    ...extra,
  };
}

beforeEach(() => {
  Object.values(ipc).forEach((mock) => {
    mock.mockReset();
  });
  ipc.getElevationStatus.mockResolvedValue({ state: 'available' });
  setLocale('en');
});

afterEach(() => setLocale('en'));

async function scanned(user: ReturnType<typeof userEvent.setup>, rules = [temp, logs]) {
  render(<MachineCleanCard rules={rules} />);
  const button = await screen.findByRole('button', { name: 'Scan as administrator' });
  await waitFor(() => expect((button as HTMLButtonElement).disabled).toBe(false));
  await user.click(button);
}

describe('the administrator cleaning card', () => {
  it('says that deleting is permanent before anything is scanned, and names no file', async () => {
    render(<MachineCleanCard rules={[temp, logs]} />);
    expect(
      await screen.findByText(/deleted permanently: it does not go to the Recycle Bin/),
    ).toBeTruthy();
    expect(screen.getAllByText('Not scanned as administrator yet')).toHaveLength(2);
    expect(ipc.scanMachineRule).not.toHaveBeenCalled();
  });

  it('renders nothing when no rule needs administrator rights', () => {
    const { container } = render(<MachineCleanCard rules={[]} />);
    expect(container.textContent).toBe('');
  });

  it('scans every rule and previews counts and sizes before any delete', async () => {
    const user = userEvent.setup();
    ipc.scanMachineRule.mockImplementation(async (id: string) => ({
      outcome: 'done',
      scan:
        id === 'windows-temp'
          ? scanView(id, {
              confirm: true,
              skipped: '3 too new',
              skippedPhrase: phrase('scan.skipped'),
            })
          : scanView(id, { files: 0, folders: 0, bytes: 0, canClean: false }),
    }));
    await scanned(user);
    expect(await screen.findByText(/Would delete 10 files and 2 empty folders/)).toBeTruthy();
    expect(screen.getByText('Nothing to delete')).toBeTruthy();
    expect(screen.getByText('Left out: 3 too new')).toBeTruthy();
    // A folder users can write to: the helper asks once more, and the page says so up front.
    expect(screen.getByText(/Windows will ask you to confirm once more/)).toBeTruthy();
    expect(ipc.scanMachineRule).toHaveBeenCalledTimes(2);
    expect(ipc.cleanMachineRule).not.toHaveBeenCalled();
    // Only the rule with something to delete has a delete button.
    expect(screen.getAllByRole('button', { name: /Delete .* permanently/ })).toHaveLength(1);
  });

  it('asks in words that say permanent, not the Recycle Bin and no undo, then deletes the one rule', async () => {
    const user = userEvent.setup();
    ipc.scanMachineRule.mockImplementation(async (id: string) => ({
      outcome: 'done',
      scan: scanView(id, { confirm: true }),
    }));
    ipc.cleanMachineRule.mockResolvedValue(done());
    await scanned(user, [temp]);
    await user.click(
      await screen.findByRole('button', { name: 'Delete Windows temporary files permanently' }),
    );
    const dialog = await screen.findByRole('dialog');
    expect(within(dialog).getByText('Delete these files permanently?')).toBeTruthy();
    expect(
      within(dialog).getByText(/do not go to the Recycle Bin and you cannot restore them/),
    ).toBeTruthy();
    expect(within(dialog).getByText(/Answer it only if you just asked for this/)).toBeTruthy();
    // Cancelling deletes nothing.
    await user.click(within(dialog).getByRole('button', { name: 'Cancel' }));
    expect(ipc.cleanMachineRule).not.toHaveBeenCalled();

    await user.click(
      screen.getByRole('button', { name: 'Delete Windows temporary files permanently' }),
    );
    await user.click(
      within(await screen.findByRole('dialog')).getByRole('button', { name: 'Delete permanently' }),
    );
    await waitFor(() => expect(ipc.cleanMachineRule).toHaveBeenCalledWith('windows-temp'));
    expect(await screen.findByText(/Permanently deleted 9 files/)).toBeTruthy();
    // The scan is used up: no delete button until the next scan.
    expect(screen.queryByRole('button', { name: /Delete .* permanently/ })).toBeNull();
  });

  it.each([
    ['notConfirmed', 'The confirmation was not accepted, so nothing was deleted.'],
    ['blocked', /You declined this a moment ago/],
    ['timedOut', /No answer came in time/],
    ['noScan', /That scan can no longer be used/],
    ['declined', 'The administrator prompt was cancelled. Nothing was deleted.'],
  ] as const)(
    'writes the refusal %s in words and keeps nothing to delete',
    async (outcome, text) => {
      const user = userEvent.setup();
      ipc.scanMachineRule.mockResolvedValue({ outcome: 'done', scan: scanView('windows-temp') });
      ipc.cleanMachineRule.mockResolvedValue({ outcome });
      await scanned(user, [temp]);
      await user.click(await screen.findByRole('button', { name: /Delete .* permanently/ }));
      await user.click(
        within(await screen.findByRole('dialog')).getByRole('button', {
          name: 'Delete permanently',
        }),
      );
      expect(await screen.findByText(text)).toBeTruthy();
      expect(screen.queryByRole('button', { name: /Delete .* permanently/ })).toBeNull();
    },
  );

  it('lists what was left in place and says when the helper stopped early', async () => {
    const user = userEvent.setup();
    ipc.scanMachineRule.mockResolvedValue({ outcome: 'done', scan: scanView('windows-temp') });
    ipc.cleanMachineRule.mockResolvedValue(
      done({
        skippedTotal: 3,
        skipped: '1 in use or denied by Windows, 2 changed since the scan',
        skippedPhrase: phrase('scan.skipped'),
        stopped: true,
        historyIncomplete: true,
      }),
    );
    await scanned(user, [temp]);
    await user.click(await screen.findByRole('button', { name: /Delete .* permanently/ }));
    await user.click(
      within(await screen.findByRole('dialog')).getByRole('button', { name: 'Delete permanently' }),
    );
    expect(
      await screen.findByText(
        'Left in place: 1 in use or denied by Windows, 2 changed since the scan',
      ),
    ).toBeTruthy();
    expect(screen.getByText(/The helper stopped before it had looked at everything/)).toBeTruthy();
    expect(screen.getByText(/activity history could not be written/)).toBeTruthy();
  });

  it('shows a problem instead of counts and offers no delete', async () => {
    const user = userEvent.setup();
    ipc.scanMachineRule.mockResolvedValue({
      outcome: 'done',
      scan: scanView('windows-temp', {
        files: 0,
        folders: 0,
        bytes: 0,
        canClean: false,
        problem: 'This folder does not exist, so there is nothing to clean.',
        problemPhrase: phrase('machine.problem.rootMissing'),
      }),
    });
    await scanned(user, [temp]);
    expect(
      await screen.findByText('This folder does not exist, so there is nothing to clean.'),
    ).toBeTruthy();
    expect(screen.queryByRole('button', { name: /Delete .* permanently/ })).toBeNull();
  });

  it('keeps everything locked and says why when the helper is not available', async () => {
    ipc.getElevationStatus.mockResolvedValue({
      state: 'unavailable',
      reason: 'notAllUsers',
      detail: 'x',
    });
    render(<MachineCleanCard rules={[temp]} />);
    expect(
      await screen.findByText('Install Volt for all users to use administrator actions.'),
    ).toBeTruthy();
    const button = screen.getByRole('button', {
      name: 'Scan as administrator',
    }) as HTMLButtonElement;
    expect(button.disabled).toBe(true);
    expect(ipc.scanMachineRule).not.toHaveBeenCalled();
  });

  it('stops scanning when the administrator prompt is cancelled', async () => {
    const user = userEvent.setup();
    ipc.scanMachineRule.mockResolvedValue({ outcome: 'declined' });
    await scanned(user);
    expect(
      await screen.findByText('The administrator prompt was cancelled. Nothing was deleted.'),
    ).toBeTruthy();
    expect(ipc.scanMachineRule).toHaveBeenCalledTimes(1);
  });

  it('writes the question and the result in Vietnamese', async () => {
    setLocale('vi');
    const user = userEvent.setup();
    ipc.scanMachineRule.mockResolvedValue({ outcome: 'done', scan: scanView('windows-temp') });
    ipc.cleanMachineRule.mockResolvedValue(done({ messagePhrase: phrase('summary.cleanMachine') }));
    render(<MachineCleanCard rules={[temp]} />);
    const scan = await screen.findByRole('button', { name: 'Quét bằng quyền quản trị' });
    await waitFor(() => expect((scan as HTMLButtonElement).disabled).toBe(false));
    await user.click(scan);
    await user.click(await screen.findByRole('button', { name: /Xóa vĩnh viễn/ }));
    const dialog = await screen.findByRole('dialog');
    expect(within(dialog).getByText('Xóa vĩnh viễn các tệp này?')).toBeTruthy();
    expect(within(dialog).getByText(/không vào Thùng rác và bạn không thể khôi phục/)).toBeTruthy();
  });
});
