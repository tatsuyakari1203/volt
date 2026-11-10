import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type {
  CleaningRuleInfo,
  CleanRunResult,
  CleanScanResult,
  RuleScanView,
} from '../../shared/ipc';
import { ToastProvider } from '../../shared/ui';
import { CleanScreen } from './CleanScreen';
import { selectionOf, tickedByDefault, withPick } from './messages';

const ipc = vi.hoisted(() => ({
  listCleaningRules: vi.fn(),
  scanCleaning: vi.fn(),
  cancelCleaningScan: vi.fn(),
  scanCleaningRules: vi.fn(),
  onCleanScanProgress: vi.fn(),
  cleanSelectedRules: vi.fn(),
  chooseCleaningFolder: vi.fn(),
  forgetCleaningFolder: vi.fn(),
  pickScanFolder: vi.fn(),
  getRecycleBinStatus: vi.fn(),
  emptyRecycleBin: vi.fn(),
}));
vi.mock('../../shared/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../shared/ipc')>()),
  ...ipc,
}));

const base: CleaningRuleInfo = {
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
const models: CleaningRuleInfo = {
  ...base,
  id: 'huggingface-hub-cache',
  title: 'Hugging Face models',
  description: 'Models downloaded by Hugging Face libraries.',
  category: 'aiModelCache',
  categoryLabel: 'AI model caches',
  risk: 'caution',
  minAgeDays: 0,
  pickItems: true,
};
const comfy: CleaningRuleInfo = {
  ...base,
  id: 'comfyui-temp',
  title: 'ComfyUI temp folder',
  description: 'The temp folder of a ComfyUI install.',
  category: 'applicationCache',
  categoryLabel: 'Application caches',
  minAgeDays: 1,
  needsFolder: true,
};
const cargo: CleaningRuleInfo = {
  ...base,
  id: 'cargo-cache',
  title: 'Cargo registry cache',
  description: 'Crates that Cargo downloaded.',
  category: 'developerCache',
  categoryLabel: 'Developer caches',
  risk: 'caution',
  minAgeDays: 1,
  follows: [{ var: 'CARGO_HOME', source: 'The Cargo Book: Environment variables' }],
};

function view(rule: CleaningRuleInfo, extra: Partial<RuleScanView> = {}): RuleScanView {
  return {
    rule,
    files: 0,
    folders: 0,
    bytes: 0,
    truncated: false,
    skipped: null,
    lockedNote: null,
    problem: null,
    samples: [],
    items: [],
    followedBy: null,
    ...extra,
  };
}

const modelView = view(models, {
  files: 13,
  bytes: 6300,
  items: [
    { index: 0, name: 'datasets--org--data', bytes: 300, files: 1 },
    { index: 1, name: 'models--org--big', bytes: 6000, files: 12 },
  ],
});

const scanned: CleanScanResult = {
  planId: 'scan-7',
  cancelled: false,
  rules: [modelView, view(cargo, { files: 4, bytes: 2048, followedBy: 'CARGO_HOME' })],
};

const cleaned: CleanRunResult = {
  movedFiles: 12,
  movedFolders: 0,
  movedBytes: 6000,
  skippedTotal: 0,
  skipped: [],
  rules: [
    {
      id: 'huggingface-hub-cache',
      title: 'Hugging Face models',
      movedFiles: 12,
      movedBytes: 6000,
      skipped: 0,
    },
  ],
  message: 'Moved 12 files to the Recycle Bin (5.9 KB).',
  historyIncomplete: false,
};

beforeEach(() => {
  for (const mock of Object.values(ipc)) mock.mockReset();
  ipc.listCleaningRules.mockResolvedValue([models, comfy, cargo]);
  ipc.getRecycleBinStatus.mockResolvedValue({ itemCount: 0, sizeBytes: 0 });
  ipc.cancelCleaningScan.mockResolvedValue(undefined);
  ipc.onCleanScanProgress.mockResolvedValue(() => {});
});

function renderScreen() {
  return render(
    <ToastProvider>
      <CleanScreen />
    </ToastProvider>,
  );
}

describe('rules picked item by item', () => {
  it('lists the items with sizes, ticks none by default and moves only the ticked ones', async () => {
    const user = userEvent.setup();
    ipc.scanCleaning.mockResolvedValue(scanned);
    ipc.cleanSelectedRules.mockResolvedValue(cleaned);
    renderScreen();
    await user.click(await screen.findByRole('button', { name: 'Scan' }));
    const summary = await screen.findByRole('region', { name: 'Selection' });

    const list = screen.getByRole('list', { name: 'Items of Hugging Face models' });
    const boxes = within(list).getAllByRole('checkbox') as HTMLInputElement[];
    // Biggest first, and none is ticked for the user.
    expect(boxes.map((box) => box.getAttribute('aria-label'))).toEqual([
      'Take models--org--big',
      'Take datasets--org--data',
    ]);
    expect(boxes.every((box) => !box.checked)).toBe(true);
    expect(within(list).getByText('5.9 KB, 12 files')).toBeTruthy();
    // The rule has no master switch; nothing is selected, so Clean is off.
    expect(screen.queryByRole('switch', { name: 'Include Hugging Face models' })).toBeNull();
    expect(summary.textContent).toContain('Nothing is selected to move');

    await user.click(boxes[0] as HTMLInputElement);
    expect(summary.textContent).toContain('12 files (5.9 KB) selected');
    await user.click(screen.getByRole('button', { name: 'Clean selected' }));
    await user.click(screen.getByRole('button', { name: 'Move to Recycle Bin' }));
    expect(ipc.cleanSelectedRules).toHaveBeenCalledWith(
      'scan-7',
      ['huggingface-hub-cache'],
      [{ ruleId: 'huggingface-hub-cache', items: [1] }],
    );
  });

  it('is never ticked by default, and its items add to the selection only when picked', () => {
    expect(tickedByDefault(models)).toBe(false);
    expect(tickedByDefault({ ...models, risk: 'safe', category: 'temporaryFiles' })).toBe(false);
    const views = [modelView];
    expect(selectionOf(views, new Set([models.id]))).toEqual({ files: 0, folders: 0, bytes: 0 });
    const picks = withPick(new Map(), models.id, 0, true);
    expect(selectionOf(views, new Set(), picks)).toEqual({ files: 1, folders: 0, bytes: 300 });
    expect(withPick(picks, models.id, 0, false).size).toBe(0);
  });
});

describe('a folder for a rule that needs one', () => {
  it('is added from the folder dialog, shown, and removed again', async () => {
    const user = userEvent.setup();
    renderScreen();
    // The dialog was closed: nothing happens.
    ipc.pickScanFolder.mockResolvedValueOnce(null);
    await user.click(
      await screen.findByRole('button', { name: 'Add a folder for ComfyUI temp folder' }),
    );
    expect(ipc.chooseCleaningFolder).not.toHaveBeenCalled();

    ipc.pickScanFolder.mockResolvedValueOnce('D:\\AI\\ComfyUI');
    ipc.chooseCleaningFolder.mockResolvedValue(undefined);
    ipc.listCleaningRules.mockResolvedValue([
      models,
      { ...comfy, chosenFolder: 'D:\\AI\\ComfyUI' },
      cargo,
    ]);
    await user.click(screen.getByRole('button', { name: 'Add a folder for ComfyUI temp folder' }));
    expect(ipc.chooseCleaningFolder).toHaveBeenCalledWith('comfyui-temp', 'D:\\AI\\ComfyUI');
    expect(await screen.findByText('D:\\AI\\ComfyUI')).toBeTruthy();

    ipc.forgetCleaningFolder.mockResolvedValue(undefined);
    ipc.listCleaningRules.mockResolvedValue([models, comfy, cargo]);
    await user.click(
      screen.getByRole('button', { name: 'Remove the folder of ComfyUI temp folder' }),
    );
    expect(ipc.forgetCleaningFolder).toHaveBeenCalledWith('comfyui-temp');
    expect(
      await screen.findByRole('button', { name: 'Add a folder for ComfyUI temp folder' }),
    ).toBeTruthy();
  });

  it('says why a folder was refused', async () => {
    const user = userEvent.setup();
    ipc.pickScanFolder.mockResolvedValue('C:\\');
    ipc.chooseCleaningFolder.mockRejectedValue({
      kind: 'invalidInput',
      message: 'this folder cannot be used: it is too broad or a protected folder',
    });
    renderScreen();
    await user.click(
      await screen.findByRole('button', { name: 'Add a folder for ComfyUI temp folder' }),
    );
    expect((await screen.findByRole('alert')).textContent).toContain('That folder was not added.');
  });

  it('says to add a folder instead of showing a switch before one is chosen', async () => {
    const user = userEvent.setup();
    ipc.scanCleaning.mockResolvedValue({
      planId: 'scan-8',
      cancelled: false,
      rules: [view(comfy)],
    } satisfies CleanScanResult);
    renderScreen();
    await user.click(await screen.findByRole('button', { name: 'Scan' }));
    await screen.findByRole('region', { name: 'Selection' });
    expect(screen.getByText(/Add a folder to look in/)).toBeTruthy();
    expect(screen.queryByRole('switch', { name: 'Include ComfyUI temp folder' })).toBeNull();
  });
});

describe('a cache found through a variable', () => {
  it('says which variable it was found through and names the documentation', async () => {
    const user = userEvent.setup();
    ipc.scanCleaning.mockResolvedValue(scanned);
    renderScreen();
    await user.click(await screen.findByRole('button', { name: 'Scan' }));
    await screen.findByRole('region', { name: 'Selection' });
    expect(screen.getByText('Found through CARGO_HOME.')).toBeTruthy();
    expect(
      screen.getByText('CARGO_HOME (source: The Cargo Book: Environment variables)'),
    ).toBeTruthy();
  });
});
