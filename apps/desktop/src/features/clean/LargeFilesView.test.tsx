import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { setLocale } from '../../shared/i18n/translate';
import type { LargeFilesResult, LargeFileView } from '../../shared/ipc';
import { ToastProvider } from '../../shared/ui';
import { CleanScreen } from './CleanScreen';
import { selectedFiles, splitPath, visibleFiles } from './largeFiles';

const ipc = vi.hoisted(() => ({
  listCleaningRules: vi.fn(),
  scanCleaning: vi.fn(),
  cancelCleaningScan: vi.fn(),
  onCleanScanProgress: vi.fn(),
  cleanSelectedRules: vi.fn(),
  getRecycleBinStatus: vi.fn(),
  emptyRecycleBin: vi.fn(),
  scanLargeFiles: vi.fn(),
  cancelLargeFilesScan: vi.fn(),
  onLargeFilesProgress: vi.fn(),
  trashLargeFiles: vi.fn(),
  revealLargeFile: vi.fn(),
  pickScanFolder: vi.fn(),
}));
vi.mock('../../shared/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../shared/ipc')>()),
  ...ipc,
}));

const NOW = Date.UTC(2026, 10, 10);
const DAY = 86_400_000;
const GB = 1024 ** 3;

function file(index: number, path: string, sizeBytes: number, extra: Partial<LargeFileView> = {}) {
  return {
    index,
    path,
    sizeBytes,
    modifiedMs: NOW - 10 * DAY,
    group: 'other',
    ...extra,
  } as LargeFileView;
}

const files: LargeFileView[] = [
  file(0, 'D:\\Media\\film.mkv', 4 * GB, { group: 'video', modifiedMs: NOW - 400 * DAY }),
  file(1, 'D:\\Media\\backup.zip', 2 * GB, { group: 'archive' }),
  file(2, 'D:\\Media\\win.iso', GB, { group: 'diskImage', modifiedMs: null }),
];

const scanned: LargeFilesResult = {
  scanId: 'large-1',
  cancelled: false,
  root: 'D:\\Media',
  files,
  filesSeen: 5000,
  unreadable: 2,
  links: 1,
  excluded: 3,
  scannedAtMs: NOW,
};

async function openLargeFiles() {
  const user = userEvent.setup();
  render(
    <ToastProvider>
      <CleanScreen />
    </ToastProvider>,
  );
  await user.click(screen.getByRole('radio', { name: 'Large files' }));
  return user;
}

describe('large files helpers', () => {
  it('splits a path into name and folder', () => {
    expect(splitPath('D:\\Media\\film.mkv')).toEqual({ name: 'film.mkv', folder: 'D:\\Media' });
    expect(splitPath('film.mkv')).toEqual({ name: 'film.mkv', folder: '' });
  });

  it('filters by type and age, and a hidden ticked file is not selected', () => {
    expect(visibleFiles(files, 'video', 0, NOW).map((f) => f.index)).toEqual([0]);
    expect(visibleFiles(files, 'all', 365, NOW).map((f) => f.index)).toEqual([0]);
    expect(visibleFiles(files, 'all', 0, NOW)).toHaveLength(3);
    const visible = visibleFiles(files, 'archive', 0, NOW);
    expect(selectedFiles(visible, new Set([0, 1])).map((f) => f.index)).toEqual([1]);
  });
});

describe('Large files view', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    setLocale('en');
    ipc.listCleaningRules.mockResolvedValue([]);
    ipc.getRecycleBinStatus.mockResolvedValue({ itemCount: 0, sizeBytes: 0 });
    ipc.onLargeFilesProgress.mockResolvedValue(() => {});
    ipc.onCleanScanProgress.mockResolvedValue(() => {});
    ipc.scanLargeFiles.mockResolvedValue(scanned);
    ipc.revealLargeFile.mockResolvedValue(undefined);
  });

  it('scans the folder the user picked and lists the files with nothing ticked', async () => {
    ipc.pickScanFolder.mockResolvedValue('D:\\Media');
    const user = await openLargeFiles();
    await user.click(screen.getByRole('button', { name: 'Choose drive or folder...' }));
    await screen.findByText('D:\\Media');
    await user.click(screen.getByRole('button', { name: 'Scan' }));
    expect(ipc.scanLargeFiles).toHaveBeenCalledWith('D:\\Media');
    const list = await screen.findByRole('list', { name: 'Biggest files' });
    expect(within(list).getAllByRole('checkbox')).toHaveLength(3);
    for (const box of within(list).getAllByRole('checkbox'))
      expect((box as HTMLInputElement).checked).toBe(false);
    expect(screen.getByText('Nothing is selected to move.')).toBeTruthy();
    expect(screen.getByText('4.0 GB ·', { exact: false })).toBeTruthy();
    expect(screen.getByText('Change date unknown', { exact: false })).toBeTruthy();
    expect(
      (screen.getByRole('button', { name: 'Move to Recycle Bin' }) as HTMLButtonElement).disabled,
    ).toBe(true);
  });

  it('scans the system drive by default', async () => {
    const user = await openLargeFiles();
    await user.click(screen.getByRole('button', { name: 'Scan' }));
    expect(ipc.scanLargeFiles).toHaveBeenCalledWith(null);
  });

  it('filters by type', async () => {
    const user = await openLargeFiles();
    await user.click(screen.getByRole('button', { name: 'Scan' }));
    await screen.findByRole('list', { name: 'Biggest files' });
    await user.selectOptions(screen.getByLabelText('Type'), 'archive');
    const list = screen.getByRole('list', { name: 'Biggest files' });
    expect(within(list).getAllByRole('checkbox')).toHaveLength(1);
    await user.selectOptions(screen.getByLabelText('Last changed'), '30');
    expect(screen.getByText('No file matches these filters.')).toBeTruthy();
  });

  it('opens the folder of a file', async () => {
    const user = await openLargeFiles();
    await user.click(screen.getByRole('button', { name: 'Scan' }));
    await user.click(await screen.findByRole('button', { name: 'Open the folder of backup.zip' }));
    expect(ipc.revealLargeFile).toHaveBeenCalledWith('large-1', 1);
  });

  it('confirms with the count and total size, then moves only the ticked files', async () => {
    ipc.trashLargeFiles.mockResolvedValue({
      movedFiles: 2,
      movedBytes: 6 * GB,
      skippedTotal: 0,
      skipped: [],
      files: [file(0, 'D:\\Media\\win.iso', GB, { group: 'diskImage', modifiedMs: null })],
      historyIncomplete: false,
    });
    const user = await openLargeFiles();
    await user.click(screen.getByRole('button', { name: 'Scan' }));
    await user.click(await screen.findByRole('checkbox', { name: 'Select film.mkv' }));
    await user.click(screen.getByRole('checkbox', { name: 'Select backup.zip' }));
    await user.click(screen.getByRole('button', { name: 'Move to Recycle Bin' }));
    const dialog = await screen.findByRole('dialog');
    expect(within(dialog).getByText(/2 files \(6\.0 GB\) will be moved/)).toBeTruthy();
    expect(ipc.trashLargeFiles).not.toHaveBeenCalled();
    await user.click(within(dialog).getByRole('button', { name: 'Move to Recycle Bin' }));
    await waitFor(() => expect(ipc.trashLargeFiles).toHaveBeenCalledWith('large-1', [0, 1]));
    expect(await screen.findByText(/Moved 2 files to the Recycle Bin \(6\.0 GB\)/)).toBeTruthy();
    const list = screen.getByRole('list', { name: 'Biggest files' });
    expect(within(list).getAllByRole('checkbox')).toHaveLength(1);
    expect(ipc.getRecycleBinStatus.mock.calls.length).toBeGreaterThan(1);
  });

  it('shows a failed scan as a notice', async () => {
    ipc.scanLargeFiles.mockRejectedValue(new Error('boom'));
    const user = await openLargeFiles();
    await user.click(screen.getByRole('button', { name: 'Scan' }));
    expect((await screen.findByRole('alert')).textContent).toContain(
      'The scan could not run. boom',
    );
  });

  it('is written in Vietnamese too', async () => {
    setLocale('vi');
    render(
      <ToastProvider>
        <CleanScreen />
      </ToastProvider>,
    );
    const user = userEvent.setup();
    await user.click(screen.getByRole('radio', { name: 'Tệp lớn' }));
    expect(screen.getByText('Tìm các tệp lớn nhất của bạn')).toBeTruthy();
  });
});
