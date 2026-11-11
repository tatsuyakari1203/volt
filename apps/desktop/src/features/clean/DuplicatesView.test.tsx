import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { setLocale } from '../../shared/i18n/translate';
import type { DuplicateGroupView, TidyResult, TidyTrashResult } from '../../shared/ipc';
import { ToastProvider } from '../../shared/ui';
import { CleanScreen } from './CleanScreen';
import { canMark, keepOnly, markedTotals, picksOf, pruneMarks, withMark } from './tidy';

const ipc = vi.hoisted(() => ({
  listCleaningRules: vi.fn(),
  scanCleaning: vi.fn(),
  cancelCleaningScan: vi.fn(),
  scanCleaningRules: vi.fn(),
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
  scanTidy: vi.fn(),
  cancelTidyScan: vi.fn(),
  onTidyProgress: vi.fn(),
  trashDuplicates: vi.fn(),
  trashEmptyFolders: vi.fn(),
  trashBrokenShortcuts: vi.fn(),
  revealTidyItem: vi.fn(),
}));
vi.mock('../../shared/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../shared/ipc')>()),
  ...ipc,
}));

const MB = 1024 ** 2;
const NOW = Date.UTC(2026, 10, 10);
const DAY = 86_400_000;

const group: DuplicateGroupView = {
  index: 0,
  sizeBytes: 40 * MB,
  wastedBytes: 80 * MB,
  hardLinks: 1,
  files: [
    { path: 'D:\\Photos\\trip.mp4', modifiedMs: NOW - 90 * DAY },
    { path: 'D:\\Backup\\trip copy.mp4', modifiedMs: NOW - 30 * DAY },
    { path: 'D:\\Old\\trip.mp4', modifiedMs: null },
  ],
};
const other: DuplicateGroupView = {
  index: 1,
  sizeBytes: 3 * MB,
  wastedBytes: 3 * MB,
  hardLinks: 0,
  files: [
    { path: 'D:\\Photos\\a.zip', modifiedMs: NOW },
    { path: 'D:\\Backup\\a.zip', modifiedMs: NOW },
  ],
};

const scanned: TidyResult = {
  scanId: 'tidy-1',
  cancelled: false,
  roots: ['D:\\'],
  lists: {
    duplicates: [group, other],
    emptyFolders: [
      { index: 0, path: 'D:\\Photos\\empty', nested: 2 },
      { index: 1, path: 'D:\\Old\\none', nested: 0 },
    ],
    brokenShortcuts: [
      { index: 0, path: 'D:\\Desk\\Game.lnk', target: 'D:\\Games\\game.exe', sizeBytes: 1200 },
    ],
  },
  groupsOmitted: 0,
  filesSeen: 9000,
  unreadable: 2,
  links: 0,
  excluded: 1,
  cloudSkipped: 3,
  hardLinksFolded: 1,
  shortcutsSeen: 5,
  shortcutsUnchecked: 4,
  truncated: false,
  minDuplicateBytes: MB,
};

const moved = (extra: Partial<TidyTrashResult> = {}): TidyTrashResult => ({
  moved: 2,
  movedBytes: 80 * MB,
  skippedTotal: 0,
  skipped: [],
  lists: { duplicates: [other], emptyFolders: scanned.lists.emptyFolders, brokenShortcuts: [] },
  historyIncomplete: false,
  ...extra,
});

async function openTidy() {
  const user = userEvent.setup();
  render(
    <ToastProvider>
      <CleanScreen />
    </ToastProvider>,
  );
  await user.click(screen.getByRole('radio', { name: 'Duplicates' }));
  ipc.pickScanFolder.mockResolvedValue('D:\\');
  await user.click(screen.getByRole('button', { name: 'Add folder...' }));
  await screen.findByText('D:\\');
  await user.click(screen.getByRole('button', { name: 'Scan' }));
  await screen.findByRole('region', { name: 'Duplicate files' });
  return user;
}

describe('duplicate marks', () => {
  it('marks copies, keeps one, and never allows every copy to be marked', () => {
    let marks = withMark(new Map(), 0, 1, true);
    expect(canMark(marks, group, false)).toBe(true);
    marks = withMark(marks, 0, 2, true);
    // Two of three are marked: the last one cannot be.
    expect(canMark(marks, group, false)).toBe(false);
    expect(canMark(marks, group, true)).toBe(true);
    expect(picksOf(marks)).toEqual([{ group: 0, files: [1, 2] }]);
    expect(withMark(marks, 0, 1, false).get(0)).toEqual(new Set([2]));
  });

  it('keeping a copy marks the others and clears nothing else', () => {
    const marks = keepOnly(withMark(new Map(), 1, 0, true), group, 1);
    expect(marks.get(0)).toEqual(new Set([0, 2]));
    expect(marks.get(1)).toEqual(new Set([0]));
    expect(markedTotals([group, other], marks)).toEqual({ copies: 3, bytes: 83 * MB });
  });

  it('drops marks that a newer list no longer has', () => {
    const marks = new Map([
      [0, new Set([1, 2])],
      [1, new Set([0, 1])],
    ]);
    // Group 1 would have every copy marked, so it is dropped; group 0 keeps its marks.
    expect(pruneMarks(marks, [group, other]).get(0)).toEqual(new Set([1, 2]));
    expect(pruneMarks(marks, [group, other]).has(1)).toBe(false);
    expect(pruneMarks(marks, [other]).size).toBe(0);
  });
});

describe('Duplicates view', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    setLocale('en');
    ipc.listCleaningRules.mockResolvedValue([]);
    ipc.getRecycleBinStatus.mockResolvedValue({ itemCount: 0, sizeBytes: 0 });
    ipc.onCleanScanProgress.mockResolvedValue(() => {});
    ipc.onTidyProgress.mockResolvedValue(() => {});
    ipc.scanTidy.mockResolvedValue(scanned);
    ipc.revealTidyItem.mockResolvedValue(undefined);
  });

  it('scans the chosen folders and lists three separate sections with nothing marked', async () => {
    await openTidy();
    expect(ipc.scanTidy).toHaveBeenCalledWith(['D:\\']);
    const duplicates = screen.getByRole('region', { name: 'Duplicate files' });
    for (const box of within(duplicates).getAllByRole('checkbox'))
      expect((box as HTMLInputElement).checked).toBe(false);
    expect(within(duplicates).getByText('No copy is marked.')).toBeTruthy();
    expect(
      within(duplicates).getByText('3 copies, 40.0 MB each, 80.0 MB wasted.', { exact: false }),
    ).toBeTruthy();
    expect(within(duplicates).getByText('Other names of this file', { exact: false })).toBeTruthy();
    const folders = screen.getByRole('region', { name: 'Empty folders' });
    expect(within(folders).getByText('With 2 empty folders inside')).toBeTruthy();
    const shortcuts = screen.getByRole('region', { name: 'Shortcuts with a missing target' });
    expect(within(shortcuts).getByText('Points to D:\\Games\\game.exe')).toBeTruthy();
    expect(
      screen.getByText('3 cloud-only files were not downloaded or compared.', { exact: false }),
    ).toBeTruthy();
    expect(screen.getByText('4 shortcuts could not be judged', { exact: false })).toBeTruthy();
  });

  it('keeps the chosen copy, marks the others, and shows a preview before moving', async () => {
    const user = await openTidy();
    ipc.trashDuplicates.mockResolvedValue(moved());
    await user.click(
      screen.getAllByRole('button', {
        name: 'Keep trip.mp4 and mark the other copies',
      })[0] as HTMLElement,
    );
    expect(
      (screen.getByRole('checkbox', { name: 'Remove trip copy.mp4' }) as HTMLInputElement).checked,
    ).toBe(true);
    const same = screen.getAllByRole('checkbox', { name: 'Remove trip.mp4' }) as HTMLInputElement[];
    expect([same[0]?.checked, same[1]?.checked]).toEqual([false, true]);
    await screen.findByText('2 files (80.0 MB) marked.', { exact: false });
    await user.click(
      screen.getAllByRole('button', { name: 'Move to Recycle Bin' })[0] as HTMLElement,
    );
    const dialog = await screen.findByRole('dialog');
    expect(within(dialog).getByText('D:\\Backup\\trip copy.mp4')).toBeTruthy();
    expect(within(dialog).getAllByText('40.0 MB')).toHaveLength(2);
    expect(
      within(dialog).getByText('Each group keeps at least one copy', { exact: false }),
    ).toBeTruthy();
    await user.click(within(dialog).getByRole('button', { name: 'Move to Recycle Bin' }));
    await waitFor(() =>
      expect(ipc.trashDuplicates).toHaveBeenCalledWith('tidy-1', [{ group: 0, files: [1, 2] }]),
    );
    await screen.findByText('Moved 2 items to the Recycle Bin (80.0 MB).', { exact: false });
  });

  it('cannot mark every copy of a group', async () => {
    const user = await openTidy();
    const boxes = screen.getAllByRole('checkbox', { name: 'Remove a.zip' }) as HTMLInputElement[];
    await user.click(boxes[0] as HTMLElement);
    expect((boxes[1] as HTMLInputElement).disabled).toBe(true);
    expect((boxes[1] as HTMLInputElement).title).toBe('The last copy that is not marked stays.');
  });

  it('moves the selected empty folders and shortcuts, each after its own preview', async () => {
    const user = await openTidy();
    ipc.trashEmptyFolders.mockResolvedValue(
      moved({ moved: 2, movedBytes: 0, lists: { ...scanned.lists, emptyFolders: [] } }),
    );
    const folders = screen.getByRole('region', { name: 'Empty folders' });
    await user.click(within(folders).getByRole('button', { name: 'Select all' }));
    await user.click(within(folders).getByRole('button', { name: 'Move to Recycle Bin' }));
    const dialog = await screen.findByRole('dialog');
    expect(within(dialog).getByText('D:\\Photos\\empty')).toBeTruthy();
    await user.click(within(dialog).getByRole('button', { name: 'Move to Recycle Bin' }));
    await waitFor(() => expect(ipc.trashEmptyFolders).toHaveBeenCalledWith('tidy-1', [0, 1]));
    await screen.findByText('Moved 2 items to the Recycle Bin.');

    ipc.trashBrokenShortcuts.mockResolvedValue(
      moved({
        moved: 1,
        movedBytes: 1200,
        lists: { ...scanned.lists, emptyFolders: [], brokenShortcuts: [] },
      }),
    );
    await user.click(screen.getByRole('checkbox', { name: 'Select Game.lnk' }));
    await user.click(
      within(screen.getByRole('region', { name: 'Shortcuts with a missing target' })).getByRole(
        'button',
        {
          name: 'Move to Recycle Bin',
        },
      ),
    );
    const second = await screen.findByRole('dialog');
    await user.click(within(second).getByRole('button', { name: 'Move to Recycle Bin' }));
    await waitFor(() => expect(ipc.trashBrokenShortcuts).toHaveBeenCalledWith('tidy-1', [0]));
  });

  it('says what was left in place and why', async () => {
    const user = await openTidy();
    ipc.trashDuplicates.mockResolvedValue(
      moved({
        moved: 0,
        movedBytes: 0,
        skippedTotal: 2,
        skipped: [
          {
            reason: 'the copy to keep is gone or changed',
            reasonPhrase: { code: 'skip.keptCopyLost', args: {} },
            count: 2,
          },
        ],
      }),
    );
    await user.click(
      screen.getAllByRole('button', {
        name: 'Keep a.zip and mark the other copies',
      })[0] as HTMLElement,
    );
    await user.click(
      screen.getAllByRole('button', { name: 'Move to Recycle Bin' })[0] as HTMLElement,
    );
    await user.click(
      within(await screen.findByRole('dialog')).getByRole('button', {
        name: 'Move to Recycle Bin',
      }),
    );
    await screen.findByText('2 items left in place.', { exact: false });
    expect(screen.getByText('the copy to keep is gone or changed', { exact: false })).toBeTruthy();
  });

  it('shows progress while comparing and can be cancelled', async () => {
    let finish: (value: TidyResult) => void = () => {};
    ipc.scanTidy.mockReturnValue(new Promise<TidyResult>((resolve) => (finish = resolve)));
    let emit: (progress: unknown) => void = () => {};
    ipc.onTidyProgress.mockImplementation(async (handler: (progress: unknown) => void) => {
      emit = handler;
      return () => {};
    });
    ipc.cancelTidyScan.mockResolvedValue(undefined);
    const user = userEvent.setup();
    render(
      <ToastProvider>
        <CleanScreen />
      </ToastProvider>,
    );
    await user.click(screen.getByRole('radio', { name: 'Duplicates' }));
    ipc.pickScanFolder.mockResolvedValue('D:\\');
    await user.click(screen.getByRole('button', { name: 'Add folder...' }));
    await user.click(await screen.findByRole('button', { name: 'Scan' }));
    await screen.findByText('Starting the scan...');
    emit({
      stage: 'comparing',
      filesSeen: 100,
      foldersSeen: 10,
      compared: 4,
      toCompare: 12,
      bytesHashed: 3 * MB,
    });
    await screen.findByText('Comparing files of the same size... 4 of 12 done, 3.0 MB read.');
    await user.click(screen.getByRole('button', { name: 'Cancel' }));
    expect(ipc.cancelTidyScan).toHaveBeenCalled();
    finish({ ...scanned, cancelled: true, scanId: '' });
    await screen.findByText('Find duplicates and clutter');
  });

  it('writes the lists in Vietnamese', async () => {
    setLocale('vi');
    ipc.pickScanFolder.mockResolvedValue('D:\\');
    const user = userEvent.setup();
    render(
      <ToastProvider>
        <CleanScreen />
      </ToastProvider>,
    );
    await user.click(screen.getByRole('radio', { name: 'Tệp trùng' }));
    await user.click(screen.getByRole('button', { name: 'Thêm thư mục...' }));
    await user.click(await screen.findByRole('button', { name: 'Quét' }));
    await screen.findByRole('region', { name: 'Tệp trùng' });
    expect(screen.getByRole('region', { name: 'Thư mục trống' })).toBeTruthy();
    expect(screen.getAllByText('Giữ bản này', { selector: 'button' }).length).toBeGreaterThan(0);
  });
});
