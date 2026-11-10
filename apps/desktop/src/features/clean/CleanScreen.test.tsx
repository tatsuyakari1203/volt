import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { act } from 'react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type {
  CleaningRuleInfo,
  CleanRunResult,
  CleanScanProgress,
  CleanScanResult,
  RuleScanView,
} from '../../shared/ipc';
import { IpcCommandError } from '../../shared/ipc';
import { formatSize } from '../../shared/text/size';
import { ToastProvider } from '../../shared/ui';
import { CleanScreen } from './CleanScreen';
import { selectionOf, tickedByDefault } from './messages';

const ipc = vi.hoisted(() => ({
  listCleaningRules: vi.fn(),
  scanCleaning: vi.fn(),
  cancelCleaningScan: vi.fn(),
  scanCleaningRules: vi.fn(),
  onCleanScanProgress: vi.fn(),
  cleanSelectedRules: vi.fn(),
  getRecycleBinStatus: vi.fn(),
  emptyRecycleBin: vi.fn(),
}));
vi.mock('../../shared/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../shared/ipc')>()),
  ...ipc,
}));

const temp: CleaningRuleInfo = {
  id: 'user-temp',
  title: 'Temporary files',
  description: 'Files programs left in your temporary folder.',
  category: 'temporaryFiles',
  categoryLabel: 'Temporary files',
  risk: 'safe',
  needsAdministrator: false,
  minAgeDays: 7,
  pickItems: false,
  needsFolder: false,
  chosenFolder: null,
  follows: [],
};
const thumbs: CleaningRuleInfo = {
  id: 'thumbnail-cache',
  title: 'Thumbnail cache',
  description: 'Preview pictures File Explorer keeps.',
  category: 'applicationCache',
  categoryLabel: 'Application caches',
  risk: 'safe',
  needsAdministrator: false,
  minAgeDays: 0,
  pickItems: false,
  needsFolder: false,
  chosenFolder: null,
  follows: [],
};
const careful: CleaningRuleInfo = {
  id: 'careful',
  title: 'Careful cache',
  description: 'Needs a second thought.',
  category: 'applicationCache',
  categoryLabel: 'Application caches',
  risk: 'caution',
  needsAdministrator: false,
  minAgeDays: 0,
  pickItems: false,
  needsFolder: false,
  chosenFolder: null,
  follows: [],
};
const machine: CleaningRuleInfo = {
  id: 'windows-temp',
  title: 'Windows temporary files',
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
const chrome: CleaningRuleInfo = {
  id: 'chrome-cache',
  title: 'Chrome cache',
  description: 'Pages Chrome saved.',
  category: 'browserCache',
  categoryLabel: 'Browser caches',
  risk: 'safe',
  needsAdministrator: false,
  minAgeDays: 1,
  pickItems: false,
  needsFolder: false,
  chosenFolder: null,
  follows: [],
};
const allRules = [temp, machine, thumbs, careful];

function view(rule: CleaningRuleInfo, extra: Partial<RuleScanView> = {}): RuleScanView {
  return {
    rule,
    files: 0,
    folders: 0,
    bytes: 0,
    truncated: false,
    skipped: null,
    skippedPhrase: null,
    lockedNote: null,
    lockedPhrase: null,
    problem: null,
    problemPhrase: null,
    samples: [],
    items: [],
    followedBy: null,
    ...extra,
  };
}

const scanned: CleanScanResult = {
  planId: 'scan-1',
  cancelled: false,
  rules: [
    view(temp, {
      files: 1200,
      folders: 3,
      bytes: 3 * 1024 * 1024,
      skipped: '4 too new',
      samples: ['user_temp/old.tmp', 'user_temp/sub/deep.tmp'],
    }),
    view(machine, { files: 10, bytes: 4096 }),
    view(thumbs, { files: 1, bytes: 2048 }),
    view(careful, { files: 5, bytes: 1024 }),
  ],
};

const cleaned: CleanRunResult = {
  movedFiles: 1201,
  movedFolders: 3,
  movedBytes: 3 * 1024 * 1024 + 2048,
  skippedTotal: 2,
  skipped: [
    {
      reason: 'locked, in use or access denied',
      reasonPhrase: { code: 'skip.locked', args: {} },
      count: 2,
    },
  ],
  rules: [
    {
      id: 'user-temp',
      title: 'Temporary files',
      movedFiles: 1200,
      movedBytes: 3145728,
      skipped: 2,
    },
    {
      id: 'thumbnail-cache',
      title: 'Thumbnail cache',
      movedFiles: 1,
      movedBytes: 2048,
      skipped: 0,
    },
  ],
  message:
    'Moved 1,201 files and 3 empty folders to the Recycle Bin (3.0 MB). 2 items left in place. No disk space is freed until the Recycle Bin is emptied.',
  historyIncomplete: false,
};

function deferred<T>() {
  let resolve: (value: T) => void = () => {};
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

let emitProgress: (progress: CleanScanProgress) => void = () => {};

beforeEach(() => {
  for (const mock of Object.values(ipc)) mock.mockReset();
  ipc.listCleaningRules.mockResolvedValue(allRules);
  ipc.getRecycleBinStatus.mockResolvedValue({ itemCount: 12, sizeBytes: 5 * 1024 * 1024 });
  ipc.cancelCleaningScan.mockResolvedValue(undefined);
  ipc.onCleanScanProgress.mockImplementation(async (handler: typeof emitProgress) => {
    emitProgress = handler;
    return () => {};
  });
});

function renderScreen() {
  return render(
    <ToastProvider>
      <CleanScreen />
    </ToastProvider>,
  );
}

async function scanNow(user: ReturnType<typeof userEvent.setup>) {
  ipc.scanCleaning.mockResolvedValue(scanned);
  await user.click(await screen.findByRole('button', { name: 'Scan' }));
  await screen.findByRole('region', { name: 'Selection' });
}

describe('before a scan', () => {
  it('lists the rules by category without sizes and offers Scan', async () => {
    renderScreen();
    expect(await screen.findByRole('button', { name: 'Scan' })).toBeTruthy();
    const group = screen.getByRole('region', { name: 'Application caches' });
    expect(within(group).getByText('Thumbnail cache')).toBeTruthy();
    expect(within(group).getAllByText('Not scanned yet')).toHaveLength(2);
    expect(screen.queryByRole('switch')).toBeNull();
    expect(ipc.scanCleaning).not.toHaveBeenCalled();
  });

  it('explains a failed rules read and reads again on Try again', async () => {
    const user = userEvent.setup();
    ipc.listCleaningRules.mockRejectedValueOnce(
      new IpcCommandError({ kind: 'other', message: 'the engine is not ready' }),
    );
    renderScreen();
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('the engine is not ready');
    await user.click(within(alert).getByRole('button', { name: 'Try again' }));
    expect(await screen.findByRole('button', { name: 'Scan' })).toBeTruthy();
  });
});

describe('scanning', () => {
  it('shows progress, then sizes per rule with Safe rules that need no rights ticked', async () => {
    const user = userEvent.setup();
    const pending = deferred<CleanScanResult>();
    ipc.scanCleaning.mockReturnValue(pending.promise);
    renderScreen();
    await user.click(await screen.findByRole('button', { name: 'Scan' }));
    expect(screen.getByRole('status').textContent).toContain('Starting the scan');

    act(() => emitProgress({ done: 2, total: 4 }));
    expect(screen.getByRole('status').textContent).toContain('Checking rule 3 of 4');
    expect(
      screen.getByRole('progressbar', { name: 'Scan progress' }).getAttribute('aria-valuenow'),
    ).toBe('2');

    pending.resolve(scanned);
    await screen.findByRole('region', { name: 'Selection' });
    expect(screen.getByText('1,200 files, 3.0 MB and 3 empty folders')).toBeTruthy();
    expect(screen.getByText('Left out: 4 too new')).toBeTruthy();

    const isOn = (name: string) =>
      screen.getByRole('switch', { name }).getAttribute('aria-checked') === 'true';
    expect(isOn('Include Temporary files')).toBe(true);
    expect(isOn('Include Thumbnail cache')).toBe(true);
    // Caution is not ticked by default, and the rule that needs rights cannot be ticked at all.
    expect(isOn('Include Careful cache')).toBe(false);
    const locked = screen.getByRole('switch', { name: 'Include Windows temporary files' });
    expect(locked.getAttribute('aria-checked')).toBe('false');
    expect((locked as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByText('Needs administrator rights')).toBeTruthy();

    const summary = screen.getByRole('region', { name: 'Selection' });
    expect(summary.textContent).toContain('1,201 files and 3 empty folders (3.0 MB) selected');
    expect(summary.textContent).toContain('moved to the Recycle Bin, not deleted');
  });

  it('shows rule-relative examples and never a full path', async () => {
    const user = userEvent.setup();
    renderScreen();
    await scanNow(user);
    expect(screen.getByText('user_temp/sub/deep.tmp')).toBeTruthy();
    expect(document.body.textContent).not.toMatch(/[A-Z]:\\/);
  });

  it('cancelling asks the shell to stop and goes back to what was there', async () => {
    const user = userEvent.setup();
    const pending = deferred<CleanScanResult>();
    ipc.scanCleaning.mockReturnValue(pending.promise);
    renderScreen();
    await user.click(await screen.findByRole('button', { name: 'Scan' }));
    await user.click(screen.getByRole('button', { name: 'Cancel' }));
    expect(ipc.cancelCleaningScan).toHaveBeenCalledTimes(1);
    expect(screen.getByRole('status').textContent).toContain('Stopping');

    pending.resolve({ planId: '', cancelled: true, rules: [] });
    expect(await screen.findByRole('button', { name: 'Scan' })).toBeTruthy();
    expect(screen.queryByRole('region', { name: 'Selection' })).toBeNull();
  });

  it('a scan cancelled inside a rule shows what it reached and says it is partial', async () => {
    const user = userEvent.setup();
    const pending = deferred<CleanScanResult>();
    ipc.scanCleaning.mockReturnValue(pending.promise);
    renderScreen();
    await user.click(await screen.findByRole('button', { name: 'Scan' }));
    await user.click(screen.getByRole('button', { name: 'Cancel' }));

    pending.resolve({
      planId: 'scan-2',
      cancelled: true,
      rules: [view(temp, { files: 7, bytes: 7 * 1024, truncated: true })],
    });
    const summary = await screen.findByRole('region', { name: 'Selection' });
    expect(summary.textContent).toContain('The scan was stopped: 1 of 4 rules were looked at');
    expect(screen.getByText(/there is more than this scan lists/)).toBeTruthy();
    // What was reached can be cleaned after the usual confirmation.
    expect(
      (within(summary).getByRole('button', { name: 'Clean selected' }) as HTMLButtonElement)
        .disabled,
    ).toBe(false);
  });

  it('says so when the scan fails', async () => {
    const user = userEvent.setup();
    ipc.scanCleaning.mockRejectedValue(
      new IpcCommandError({ kind: 'invalidInput', message: 'a scan is already running' }),
    );
    renderScreen();
    await user.click(await screen.findByRole('button', { name: 'Scan' }));
    expect((await screen.findByRole('alert')).textContent).toContain('a scan is already running');
  });
});

describe('choosing and cleaning', () => {
  it('unticking a rule changes the totals, and Clean is off when nothing is ticked', async () => {
    const user = userEvent.setup();
    renderScreen();
    await scanNow(user);
    await user.click(screen.getByRole('switch', { name: 'Include Temporary files' }));
    expect(screen.getByRole('region', { name: 'Selection' }).textContent).toContain(
      '1 file (2.0 KB) selected',
    );
    await user.click(screen.getByRole('switch', { name: 'Include Thumbnail cache' }));
    const summary = screen.getByRole('region', { name: 'Selection' });
    expect(summary.textContent).toContain('Nothing is selected to move');
    expect(
      (within(summary).getByRole('button', { name: 'Clean selected' }) as HTMLButtonElement)
        .disabled,
    ).toBe(true);
  });

  it('confirms with what will be MOVED to the Recycle Bin, then cleans only the ticked rules', async () => {
    const user = userEvent.setup();
    renderScreen();
    await scanNow(user);
    await user.click(screen.getByRole('button', { name: 'Clean selected' }));

    const dialog = screen.getByRole('dialog', { name: 'Move these files to the Recycle Bin?' });
    expect(dialog.textContent).toContain(
      '1,201 files and 3 empty folders (3.0 MB) will be moved to the Recycle Bin',
    );
    expect(dialog.textContent).toContain('keep using disk space until you empty the Recycle Bin');
    expect(ipc.cleanSelectedRules).not.toHaveBeenCalled();

    ipc.cleanSelectedRules.mockResolvedValue(cleaned);
    await user.click(within(dialog).getByRole('button', { name: 'Move to Recycle Bin' }));
    const result = await screen.findByRole('region', { name: 'Clean result' });
    expect(ipc.cleanSelectedRules).toHaveBeenCalledWith(
      'scan-1',
      ['user-temp', 'thumbnail-cache'],
      [],
    );

    // The result reads as a move, with reasons, and never claims space was freed.
    expect(result.textContent).toContain('Moved to the Recycle Bin');
    expect(result.textContent).toContain('locked, in use or access denied');
    expect(result.textContent).not.toMatch(/\bfreed (up|\d)|\bfreed\b(?! until)/i);
    // The scan is used up: no stale list can be cleaned twice.
    expect(screen.queryByRole('region', { name: 'Selection' })).toBeNull();
    // The Recycle Bin card re-reads the bin.
    await waitFor(() => expect(ipc.getRecycleBinStatus).toHaveBeenCalledTimes(2));
  });

  it('after cleaning a truncated rule it offers the rest, and scans only that rule', async () => {
    const user = userEvent.setup();
    renderScreen();
    ipc.scanCleaning.mockResolvedValue({
      ...scanned,
      rules: [
        view(temp, { files: 1000, bytes: 1024, truncated: true }),
        view(thumbs, { files: 1, bytes: 2048 }),
      ],
    });
    await user.click(await screen.findByRole('button', { name: 'Scan' }));
    await screen.findByRole('region', { name: 'Selection' });
    ipc.cleanSelectedRules.mockResolvedValue(cleaned);
    await user.click(screen.getByRole('button', { name: 'Clean selected' }));
    await user.click(
      within(screen.getByRole('dialog')).getByRole('button', { name: 'Move to Recycle Bin' }),
    );
    const result = await screen.findByRole('region', { name: 'Clean result' });
    expect(result.textContent).toContain('nothing moves until you confirm');

    ipc.scanCleaningRules.mockResolvedValue({
      planId: 'scan-3',
      cancelled: false,
      rules: [view(temp, { files: 40, bytes: 4096 })],
    });
    await user.click(within(result).getByRole('button', { name: 'Scan the rest' }));
    const summary = await screen.findByRole('region', { name: 'Selection' });
    expect(ipc.scanCleaningRules).toHaveBeenCalledWith(['user-temp']);
    // A scan, not a clean: the files wait for the confirmation.
    expect(ipc.cleanSelectedRules).toHaveBeenCalledTimes(1);
    expect(summary.textContent).toContain('40 files');
  });

  it('offers no rest after a clean of rules that were fully listed', async () => {
    const user = userEvent.setup();
    renderScreen();
    await scanNow(user);
    ipc.cleanSelectedRules.mockResolvedValue(cleaned);
    await user.click(screen.getByRole('button', { name: 'Clean selected' }));
    await user.click(
      within(screen.getByRole('dialog')).getByRole('button', { name: 'Move to Recycle Bin' }),
    );
    await screen.findByRole('region', { name: 'Clean result' });
    expect(screen.queryByRole('button', { name: 'Scan the rest' })).toBeNull();
  });

  it('the confirmation repeats that a running browser keeps files open, only when it applies', async () => {
    const user = userEvent.setup();
    ipc.listCleaningRules.mockResolvedValue([...allRules, chrome]);
    renderScreen();
    ipc.scanCleaning.mockResolvedValue({
      ...scanned,
      rules: [
        ...scanned.rules,
        view(chrome, {
          files: 9,
          bytes: 9000,
          lockedNote: 'chrome is running: files it keeps open are skipped.',
        }),
      ],
    });
    await user.click(await screen.findByRole('button', { name: 'Scan' }));
    await screen.findByRole('region', { name: 'Selection' });

    // Browser caches are not ticked by default: no note while the browser rule is off.
    await user.click(screen.getByRole('button', { name: 'Clean selected' }));
    expect(screen.queryByRole('note')).toBeNull();
    await user.click(within(screen.getByRole('dialog')).getByRole('button', { name: 'Cancel' }));

    await user.click(screen.getByRole('switch', { name: 'Include Chrome cache' }));
    await user.click(screen.getByRole('button', { name: 'Clean selected' }));
    const note = within(screen.getByRole('dialog')).getByRole('note');
    expect(note.textContent).toContain('Chrome cache');
    expect(note.textContent).toContain('files it keeps open will be skipped');
  });

  it('cancelling the confirmation moves nothing', async () => {
    const user = userEvent.setup();
    renderScreen();
    await scanNow(user);
    await user.click(screen.getByRole('button', { name: 'Clean selected' }));
    await user.click(within(screen.getByRole('dialog')).getByRole('button', { name: 'Cancel' }));
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(ipc.cleanSelectedRules).not.toHaveBeenCalled();
  });

  it('reports a refused clean and drops a scan that is out of date', async () => {
    const user = userEvent.setup();
    renderScreen();
    await scanNow(user);
    ipc.cleanSelectedRules.mockRejectedValue(
      new IpcCommandError({ kind: 'notFound', message: 'that scan is out of date' }),
    );
    await user.click(screen.getByRole('button', { name: 'Clean selected' }));
    await user.click(
      within(screen.getByRole('dialog')).getByRole('button', { name: 'Move to Recycle Bin' }),
    );
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('Nothing was moved');
    expect(alert.textContent).toContain('out of date');
    expect(screen.queryByRole('region', { name: 'Selection' })).toBeNull();
    expect(await screen.findByRole('button', { name: 'Scan' })).toBeTruthy();
  });
});

describe('the Recycle Bin card', () => {
  it('shows what the bin holds without any action being taken', async () => {
    renderScreen();
    const card = await screen.findByRole('region', { name: 'Recycle Bin' });
    expect(await within(card).findByText(/12 items, 5.0 MB/)).toBeTruthy();
    expect(ipc.emptyRecycleBin).not.toHaveBeenCalled();
  });

  it('asks first, says it is permanent, and empties only after the confirmation', async () => {
    const user = userEvent.setup();
    renderScreen();
    const card = await screen.findByRole('region', { name: 'Recycle Bin' });
    await within(card).findByText(/12 items/);
    await user.click(within(card).getByRole('button', { name: 'Empty Recycle Bin' }));

    const dialog = screen.getByRole('dialog', { name: 'Empty the Recycle Bin for good?' });
    expect(dialog.textContent).toContain('permanently deletes 12 items (5.0 MB)');
    expect(dialog.textContent).toContain('cannot be undone');
    expect(ipc.emptyRecycleBin).not.toHaveBeenCalled();

    // Cancel first: nothing is deleted.
    await user.click(within(dialog).getByRole('button', { name: 'Cancel' }));
    expect(ipc.emptyRecycleBin).not.toHaveBeenCalled();

    ipc.emptyRecycleBin.mockResolvedValue({
      emptied: true,
      itemCount: 12,
      sizeBytes: 5 * 1024 * 1024,
      historyIncomplete: false,
    });
    ipc.getRecycleBinStatus.mockResolvedValue({ itemCount: 0, sizeBytes: 0 });
    await user.click(within(card).getByRole('button', { name: 'Empty Recycle Bin' }));
    await user.click(
      within(screen.getByRole('dialog')).getByRole('button', { name: 'Delete permanently' }),
    );
    expect(ipc.emptyRecycleBin).toHaveBeenCalledTimes(1);
    expect(await screen.findByText(/12 items \(5.0 MB\) were permanently deleted/)).toBeTruthy();
    expect(await within(card).findByText('The Recycle Bin is empty.')).toBeTruthy();
  });

  it('cannot be emptied when it is empty', async () => {
    ipc.getRecycleBinStatus.mockResolvedValue({ itemCount: 0, sizeBytes: 0 });
    renderScreen();
    const card = await screen.findByRole('region', { name: 'Recycle Bin' });
    await within(card).findByText('The Recycle Bin is empty.');
    expect(
      (within(card).getByRole('button', { name: 'Empty Recycle Bin' }) as HTMLButtonElement)
        .disabled,
    ).toBe(true);
  });

  it('explains a failed emptying and a failed read', async () => {
    const user = userEvent.setup();
    ipc.emptyRecycleBin.mockRejectedValue(
      new IpcCommandError({ kind: 'other', message: 'the shell refused' }),
    );
    renderScreen();
    const card = await screen.findByRole('region', { name: 'Recycle Bin' });
    await within(card).findByText(/12 items/);
    await user.click(within(card).getByRole('button', { name: 'Empty Recycle Bin' }));
    await user.click(
      within(screen.getByRole('dialog')).getByRole('button', { name: 'Delete permanently' }),
    );
    expect((await within(card).findByRole('alert')).textContent).toContain('the shell refused');
  });
});

describe('helpers', () => {
  it('writes sizes like the core does', () => {
    expect(formatSize(0)).toBe('0 B');
    expect(formatSize(1023)).toBe('1023 B');
    expect(formatSize(1536)).toBe('1.5 KB');
    expect(formatSize(3 * 1024 * 1024)).toBe('3.0 MB');
    expect(formatSize(Number.NaN)).toBe('—');
  });

  it('ticks only Safe rules that need no administrator rights', () => {
    expect(tickedByDefault(temp)).toBe(true);
    expect(tickedByDefault(careful)).toBe(false);
    expect(tickedByDefault({ ...temp, needsAdministrator: true })).toBe(false);
  });

  it('leaves browser caches unticked', () => {
    const chrome: CleaningRuleInfo = {
      ...temp,
      id: 'chrome-cache',
      title: 'Chrome cache',
      category: 'browserCache',
      categoryLabel: 'Browser caches',
      minAgeDays: 1,
      pickItems: false,
      needsFolder: false,
      chosenFolder: null,
      follows: [],
    };
    expect(tickedByDefault(chrome)).toBe(false);
  });

  it('leaves shader caches and chat app caches unticked', () => {
    const shaders: CleaningRuleInfo = {
      ...temp,
      id: 'directx-shader-cache',
      category: 'shaderCache',
      categoryLabel: 'Shader caches',
    };
    const chat: CleaningRuleInfo = {
      ...temp,
      id: 'zalo-cache',
      category: 'chatAppCache',
      categoryLabel: 'Chat app caches',
    };
    expect(tickedByDefault(shaders)).toBe(false);
    expect(tickedByDefault(chat)).toBe(false);
    expect(
      tickedByDefault({
        ...temp,
        id: 'npm-cache',
        category: 'developerCache',
        categoryLabel: 'Developer caches',
      }),
    ).toBe(false);
  });

  it('never counts a rule that needs administrator rights, even when it is in the set', () => {
    const total = selectionOf(scanned.rules, new Set(['user-temp', 'windows-temp']));
    expect(total.files).toBe(1200);
    expect(total.folders).toBe(3);
  });
});
