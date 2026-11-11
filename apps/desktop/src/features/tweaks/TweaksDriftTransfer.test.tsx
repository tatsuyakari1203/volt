import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type {
  DriftView,
  ImportPreview,
  ImportResult,
  PhraseView,
  TweakInfo,
  TweakStepView,
} from '../../shared/ipc';
import { ToastProvider } from '../../shared/ui';
import { TweaksScreen } from './TweaksScreen';

const ipc = vi.hoisted(() => ({
  listTweaks: vi.fn(),
  listTweakDrift: vi.fn(),
  applyTweak: vi.fn(),
  acceptTweakState: vi.fn(),
  exportConfiguration: vi.fn(),
  previewConfigurationImport: vi.fn(),
  applyConfigurationImport: vi.fn(),
  revertTweak: vi.fn(),
  getTweakPlan: vi.fn(),
  resetTweak: vi.fn(),
}));
vi.mock('../../shared/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../shared/ipc')>()),
  ...ipc,
}));

function raw(text: string): PhraseView {
  return { code: 'preview.text', args: { text: { kind: 'text', value: text } } };
}

function step(subject: string, current: string, target: string, atTarget: boolean): TweakStepView {
  return {
    subject,
    current,
    target,
    subjectPhrase: raw(subject),
    currentPhrase: raw(current),
    targetPhrase: raw(target),
    atTarget,
  };
}

const flag: TweakInfo = {
  id: 'show-file-extensions',
  title: 'Show file extensions',
  description: 'Always show file endings.',
  category: 'appearance',
  risk: 'safe',
  evidence: 'anecdotal',
  evidenceNote: 'A preference.',
  restart: 'none',
  needsAdmin: false,
  state: { kind: 'notApplied' },
  appliedByVolt: true,
};
const other: TweakInfo = { ...flag, id: 'hide-tips', title: 'Hide tips' };

const drifted: DriftView = {
  id: flag.id,
  state: { kind: 'notApplied' },
  steps: [step('HKCU\\Software\\X\\Hide', 'dword:1', 'dword:0', false)],
};

function renderScreen() {
  return render(
    <ToastProvider>
      <TweaksScreen />
    </ToastProvider>,
  );
}

beforeEach(() => {
  for (const fn of Object.values(ipc)) fn.mockReset();
  ipc.listTweaks.mockResolvedValue([flag, other]);
  ipc.listTweakDrift.mockResolvedValue([]);
});

describe('tweaks changed behind the back of Volt', () => {
  it('lists nothing when no tweak drifted, and a card with what changed when one did', async () => {
    renderScreen();
    await screen.findByRole('switch', { name: 'Show file extensions' });
    expect(screen.queryByRole('region', { name: 'Changed since Volt turned them on' })).toBeNull();
  });

  it('names the tweak, says how much of it is left and shows the changed values', async () => {
    ipc.listTweakDrift.mockResolvedValue([drifted]);
    renderScreen();
    const card = await screen.findByRole('region', { name: 'Changed since Volt turned them on' });
    expect(card.textContent).toContain('1 tweak that Volt turned on has been changed since');
    expect(card.textContent).toContain('Show file extensions');
    expect(card.textContent).toContain('None of it is in place any more.');
    const changes = within(card).getByRole('list', {
      name: 'What changed in Show file extensions',
    });
    expect(changes.textContent).toContain('HKCU\\Software\\X\\Hide');
    expect(changes.textContent).toContain('dword:1');
  });

  it('turns the tweak on again through a normal apply and drops it from the card', async () => {
    ipc.listTweakDrift.mockResolvedValue([drifted]);
    ipc.applyTweak.mockResolvedValue({
      tweak: flag.id,
      status: 'applied',
      detail: null,
      state: { kind: 'applied' },
    });
    const user = userEvent.setup();
    renderScreen();
    await user.click(
      await screen.findByRole('button', { name: 'Turn Show file extensions on again' }),
    );
    expect(ipc.applyTweak).toHaveBeenCalledWith(flag.id);
    await waitFor(() =>
      expect(
        screen.queryByRole('region', { name: 'Changed since Volt turned them on' }),
      ).toBeNull(),
    );
  });

  it('asks before keeping the new state and says what happens to the undo record', async () => {
    ipc.listTweakDrift.mockResolvedValue([drifted]);
    ipc.acceptTweakState.mockResolvedValue({
      tweak: flag.id,
      accepted: 1,
      state: { kind: 'notApplied' },
    });
    const user = userEvent.setup();
    renderScreen();
    await user.click(
      await screen.findByRole('button', { name: 'Keep the new state of Show file extensions' }),
    );
    const dialog = await screen.findByRole('dialog', { name: 'Keep the new state?' });
    expect(dialog.textContent).toContain('you can no longer undo it from there');
    expect(ipc.acceptTweakState).not.toHaveBeenCalled();

    await user.click(within(dialog).getByRole('button', { name: 'Cancel' }));
    expect(ipc.acceptTweakState).not.toHaveBeenCalled();

    await user.click(
      await screen.findByRole('button', { name: 'Keep the new state of Show file extensions' }),
    );
    await user.click(
      within(await screen.findByRole('dialog')).getByRole('button', { name: 'Keep new state' }),
    );
    expect(ipc.acceptTweakState).toHaveBeenCalledWith(flag.id);
    await waitFor(() =>
      expect(
        screen.queryByRole('region', { name: 'Changed since Volt turned them on' }),
      ).toBeNull(),
    );
    expect(
      await screen.findByText(/no longer treats "Show file extensions" as turned on/),
    ).toBeTruthy();
  });

  it('still lists the tweaks when the drift check fails', async () => {
    ipc.listTweakDrift.mockRejectedValue(new Error('boom'));
    renderScreen();
    expect(await screen.findByRole('switch', { name: 'Show file extensions' })).toBeTruthy();
    expect(screen.queryByRole('region', { name: 'Changed since Volt turned them on' })).toBeNull();
  });
});

const preview: ImportPreview = {
  tweaks: [
    {
      id: flag.id,
      title: flag.title,
      action: {
        kind: 'apply',
        steps: [step('HKCU\\Software\\X\\Flag', '(not set)', 'dword:0', false)],
      },
    },
    { id: 'hide-tips', title: 'Hide tips', action: { kind: 'alreadyApplied' } },
    {
      id: 'machine-wide',
      title: 'Machine wide',
      action: { kind: 'skip', reason: 'needsAdministrator', detail: null },
    },
    { id: 'made-up', title: null, action: { kind: 'skip', reason: 'unknown', detail: null } },
  ],
  settings: [
    {
      key: 'boost.config',
      status: 'change',
      current: 'Boost runs working sets. Move temporary files to the Recycle Bin: off.',
      currentPhrase: null,
      new: 'Boost runs working sets, standby list. Move temporary files to the Recycle Bin: off.',
      newPhrase: null,
    },
    {
      key: 'memory.exclusions',
      status: 'same',
      current: null,
      currentPhrase: null,
      new: null,
      newPhrase: null,
    },
  ],
  ignoredSettings: 2,
};

const imported: ImportResult = {
  tweaks: [
    { id: flag.id, title: flag.title, status: 'applied', detail: null },
    { id: 'hide-tips', title: 'Hide tips', status: 'alreadyApplied', detail: null },
  ],
  settingsStored: 1,
  settingsFailed: 0,
};

describe('export and import', () => {
  it('exports and says where the file went and what it holds', async () => {
    ipc.exportConfiguration.mockResolvedValue({
      kind: 'saved',
      path: 'C:\\Users\\me\\volt.json',
      tweaks: 3,
      settings: 2,
    });
    const user = userEvent.setup();
    renderScreen();
    await user.click(await screen.findByRole('button', { name: 'Export...' }));
    expect(
      await screen.findByText('Saved to C:\\Users\\me\\volt.json. Tweaks: 3. Settings: 2.'),
    ).toBeTruthy();
  });

  it('says what is left out of the file and why', async () => {
    renderScreen();
    const card = await screen.findByRole('region', { name: 'Move to another PC' });
    expect(card.textContent).toContain('folders added to cleaning rules');
    expect(card.textContent).toContain('by file name only, never by path');
  });

  it('shows the whole preview, applies nothing until confirmed and then imports once', async () => {
    ipc.previewConfigurationImport.mockResolvedValue({
      kind: 'preview',
      fileName: 'volt.json',
      preview,
    });
    ipc.applyConfigurationImport.mockResolvedValue(imported);
    const user = userEvent.setup();
    renderScreen();
    await user.click(await screen.findByRole('button', { name: 'Import...' }));
    const dialog = await screen.findByRole('dialog', { name: 'Import this configuration?' });

    const toTurnOn = within(dialog).getByRole('region', { name: 'Tweaks to turn on' });
    expect(toTurnOn.textContent).toContain('Show file extensions');
    expect(toTurnOn.textContent).toContain('HKCU\\Software\\X\\Flag');
    expect(within(dialog).getByRole('region', { name: 'Already on here' }).textContent).toContain(
      'Hide tips',
    );
    const left = within(dialog).getByRole('region', { name: 'Left out' });
    expect(left.textContent).toContain('Needs administrator rights');
    expect(left.textContent).toContain('made-up');
    expect(left.textContent).toContain('no tweak with this id');
    const settings = within(dialog).getByRole('region', { name: 'Settings' });
    expect(settings.textContent).toContain('standby list');
    expect(settings.textContent).toContain('Now: Boost runs working sets.');
    expect(settings.textContent).toContain('Already the same here.');
    expect(dialog.textContent).toContain('2 other settings in the file are ignored');
    expect(ipc.applyConfigurationImport).not.toHaveBeenCalled();

    await user.click(within(dialog).getByRole('button', { name: 'Import' }));
    expect(ipc.applyConfigurationImport).toHaveBeenCalledTimes(1);
    expect(
      await screen.findByText('Imported. Tweaks turned on: 1. Settings stored: 1.'),
    ).toBeTruthy();
    // The page reads the system again.
    await waitFor(() => expect(ipc.listTweaks.mock.calls.length).toBeGreaterThan(1));
  });

  it('applies nothing when the preview is cancelled', async () => {
    ipc.previewConfigurationImport.mockResolvedValue({
      kind: 'preview',
      fileName: 'volt.json',
      preview,
    });
    const user = userEvent.setup();
    renderScreen();
    await user.click(await screen.findByRole('button', { name: 'Import...' }));
    const dialog = await screen.findByRole('dialog');
    await user.click(within(dialog).getByRole('button', { name: 'Cancel' }));
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(ipc.applyConfigurationImport).not.toHaveBeenCalled();
  });

  it('cannot import a file that would change nothing', async () => {
    ipc.previewConfigurationImport.mockResolvedValue({
      kind: 'preview',
      fileName: 'volt.json',
      preview: { tweaks: [], settings: [], ignoredSettings: 0 },
    });
    const user = userEvent.setup();
    renderScreen();
    await user.click(await screen.findByRole('button', { name: 'Import...' }));
    const dialog = await screen.findByRole('dialog');
    expect(dialog.textContent).toContain('Nothing in this file would change this PC.');
    expect(
      (within(dialog).getByRole('button', { name: 'Import' }) as HTMLButtonElement).disabled,
    ).toBe(true);
  });

  it('does nothing when the file dialog is closed and reports a file that cannot be used', async () => {
    ipc.previewConfigurationImport.mockResolvedValueOnce({ kind: 'cancelled' });
    const user = userEvent.setup();
    renderScreen();
    await user.click(await screen.findByRole('button', { name: 'Import...' }));
    await waitFor(() => expect(ipc.previewConfigurationImport).toHaveBeenCalledTimes(1));
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(screen.queryByRole('alert')).toBeNull();

    ipc.previewConfigurationImport.mockRejectedValueOnce(
      new Error('this is not a Volt configuration file'),
    );
    await user.click(screen.getByRole('button', { name: 'Import...' }));
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('Could not use that file.');
    expect(alert.textContent).toContain('not a Volt configuration file');
  });

  it('reports a tweak that could not be turned on as a problem, not a success', async () => {
    ipc.previewConfigurationImport.mockResolvedValue({
      kind: 'preview',
      fileName: 'volt.json',
      preview,
    });
    ipc.applyConfigurationImport.mockResolvedValue({
      tweaks: [{ id: flag.id, title: flag.title, status: 'rolledBack', detail: 'access denied' }],
      settingsStored: 0,
      settingsFailed: 0,
    });
    const user = userEvent.setup();
    renderScreen();
    await user.click(await screen.findByRole('button', { name: 'Import...' }));
    await user.click(
      within(await screen.findByRole('dialog')).getByRole('button', { name: 'Import' }),
    );
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('Imported with problems.');
    expect(alert.textContent).toContain('Could not turn on: 1');
  });
});
