import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type {
  ApplyResult,
  PhraseView,
  RevertResult,
  TweakInfo,
  TweakPlanView,
  TweakStateView,
  TweakStepView,
} from '../../shared/ipc';
import { IpcCommandError, IpcUnavailableError } from '../../shared/ipc';
import { ToastProvider } from '../../shared/ui';
import { MODE_STORAGE_KEY, TweaksScreen } from './TweaksScreen';

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
  restart: 'none',
  needsAdmin: false,
  state: { kind: 'notApplied' },
  appliedByVolt: false,
};

const advanced: TweakInfo = {
  ...fileExtensions,
  id: 'sysmain',
  title: 'Stop the SysMain service',
  description: 'Disables prefetching.',
  risk: 'advanced',
  evidence: 'measured',
  evidenceNote: 'Benchmarked.',
};

function applied(state: TweakStateView = { kind: 'applied' }): ApplyResult {
  return { tweak: fileExtensions.id, status: 'applied', detail: null, detailPhrase: null, state };
}

function reverted(state: TweakStateView = { kind: 'notApplied' }): RevertResult {
  return {
    tweak: fileExtensions.id,
    status: 'reverted',
    restored: 1,
    failed: 0,
    detail: null,
    detailPhrase: null,
    state,
  };
}

/** A promise the test settles by hand, to observe the state while a command runs. */
function deferred<T>() {
  let resolve: (value: T) => void = () => {};
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

/** A text that is the same in every language, as the core sends it. */
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

function plan(over: Partial<TweakPlanView> = {}): TweakPlanView {
  return {
    id: 'x',
    steps: [step('HKCU\\Software\\X\\Flag', '(not set)', 'dword:0', false)],
    reset: null,
    ...over,
  };
}

function advancedMode() {
  window.localStorage.setItem(MODE_STORAGE_KEY, 'advanced');
}

function renderScreen() {
  return render(
    <ToastProvider>
      <TweaksScreen />
    </ToastProvider>,
  );
}

async function theSwitch(name = 'Show file extensions') {
  return screen.findByRole('switch', { name });
}

beforeEach(() => {
  ipc.listTweaks.mockReset();
  ipc.applyTweak.mockReset();
  ipc.revertTweak.mockReset();
  ipc.getTweakPlan.mockReset();
  ipc.resetTweak.mockReset();
  ipc.getTweakPlan.mockResolvedValue(plan());
});

describe('loading, empty and error states', () => {
  it('shows a loading state, then the list read from the shell', async () => {
    const pending = deferred<TweakInfo[]>();
    ipc.listTweaks.mockReturnValue(pending.promise);
    renderScreen();
    expect(screen.getByRole('status').textContent).toContain('Reading your settings');
    expect(screen.queryByRole('list', { name: 'Appearance tweaks' })).toBeNull();

    pending.resolve([fileExtensions]);
    const list = await screen.findByRole('list', { name: 'Appearance tweaks' });
    expect(within(list).getAllByRole('listitem')).toHaveLength(1);
    expect(screen.queryByRole('status')).toBeNull();
  });

  it('shows the real labels: risk, honest evidence and the note', async () => {
    advancedMode();
    ipc.listTweaks.mockResolvedValue([fileExtensions]);
    renderScreen();
    const row = (await screen.findByRole('listitem')) as HTMLElement;
    expect(within(row).getByText('Safe')).toBeTruthy();
    expect(row.textContent).toContain('Evidence: Anecdotal. A usability and safety preference');
    expect(screen.queryByText('Sample data')).toBeNull();
  });

  it('shows a tweak that needs administrator rights disabled, with a label', async () => {
    advancedMode();
    ipc.listTweaks.mockResolvedValue([
      fileExtensions,
      { ...fileExtensions, id: 'machine-wide', title: 'Machine wide', needsAdmin: true },
    ]);
    renderScreen();
    const machineWide = await theSwitch('Machine wide');
    expect((machineWide as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByText(/Needs administrator rights/)).toBeTruthy();
    expect(((await theSwitch()) as HTMLButtonElement).disabled).toBe(false);
  });

  it('says so when the catalogue is empty', async () => {
    ipc.listTweaks.mockResolvedValue([]);
    renderScreen();
    expect(await screen.findByText('No tweaks yet')).toBeTruthy();
    expect(screen.queryByRole('searchbox')).toBeNull();
  });

  it('explains a failed read and offers Try again, which reads again', async () => {
    const user = userEvent.setup();
    ipc.listTweaks.mockRejectedValueOnce(
      new IpcCommandError({ kind: 'other', message: 'the store is locked' }),
    );
    renderScreen();
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('Tweaks could not be loaded');
    expect(alert.textContent).toContain('the store is locked');

    ipc.listTweaks.mockResolvedValue([fileExtensions]);
    await user.click(screen.getByRole('button', { name: 'Try again' }));
    expect(await theSwitch()).toBeTruthy();
    expect(ipc.listTweaks).toHaveBeenCalledTimes(2);
  });

  it('says the screen needs the Volt app when run in a plain browser', async () => {
    ipc.listTweaks.mockRejectedValue(new IpcUnavailableError());
    renderScreen();
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('only available inside the Volt app');
    expect(alert.textContent).not.toContain('TypeError');
  });

  it('reads the state again each time the screen is created, as a new window does', async () => {
    ipc.listTweaks.mockResolvedValueOnce([fileExtensions]);
    const first = renderScreen();
    expect((await theSwitch()).getAttribute('aria-checked')).toBe('false');
    first.unmount();

    // Applied meanwhile (from another window or outside Volt): the new screen shows the truth.
    ipc.listTweaks.mockResolvedValueOnce([{ ...fileExtensions, state: { kind: 'applied' } }]);
    renderScreen();
    await vi.waitFor(async () =>
      expect((await theSwitch()).getAttribute('aria-checked')).toBe('true'),
    );
    expect(ipc.listTweaks).toHaveBeenCalledTimes(2);
    expect(ipc.applyTweak).not.toHaveBeenCalled();
  });
});

describe('turning a tweak on and off', () => {
  it('shows a pending state while the command runs, then the state the shell reports', async () => {
    const user = userEvent.setup();
    ipc.listTweaks.mockResolvedValue([fileExtensions]);
    const running = deferred<ApplyResult>();
    ipc.applyTweak.mockReturnValue(running.promise);
    renderScreen();

    const toggle = await theSwitch();
    await user.click(toggle);
    expect(ipc.applyTweak).toHaveBeenCalledWith('show-file-extensions');
    expect(toggle.getAttribute('aria-busy')).toBe('true');
    expect(toggle.hasAttribute('disabled')).toBe(true);
    expect(screen.getByText('Turning on...')).toBeTruthy();
    // Not claimed before it is true.
    expect(toggle.getAttribute('aria-checked')).toBe('false');

    running.resolve(applied());
    await vi.waitFor(() => expect(toggle.getAttribute('aria-checked')).toBe('true'));
    expect(toggle.getAttribute('aria-busy')).toBe('false');
    expect(toggle.hasAttribute('disabled')).toBe(false);
    expect(screen.queryByText('Turning on...')).toBeNull();
  });

  it('confirms with a toast whose Undo calls revert and shows the restored state', async () => {
    const user = userEvent.setup();
    ipc.listTweaks.mockResolvedValue([fileExtensions]);
    ipc.applyTweak.mockResolvedValue(applied());
    ipc.revertTweak.mockResolvedValue(reverted());
    renderScreen();

    const toggle = await theSwitch();
    await user.click(toggle);
    const toast = await screen.findByText('"Show file extensions" is on.');
    expect(toast).toBeTruthy();

    await user.click(screen.getByRole('button', { name: 'Undo' }));
    expect(ipc.revertTweak).toHaveBeenCalledTimes(1);
    expect(ipc.revertTweak).toHaveBeenCalledWith('show-file-extensions');
    await vi.waitFor(() => expect(toggle.getAttribute('aria-checked')).toBe('false'));
    expect(await screen.findByText(/Undone\./)).toBeTruthy();
  });

  it('turns an applied tweak off through revert', async () => {
    const user = userEvent.setup();
    ipc.listTweaks.mockResolvedValue([
      { ...fileExtensions, state: { kind: 'applied' }, appliedByVolt: true },
    ]);
    ipc.revertTweak.mockResolvedValue(reverted());
    renderScreen();

    const toggle = await theSwitch();
    expect(toggle.getAttribute('aria-checked')).toBe('true');
    await user.click(toggle);
    expect(ipc.revertTweak).toHaveBeenCalledWith('show-file-extensions');
    await vi.waitFor(() => expect(toggle.getAttribute('aria-checked')).toBe('false'));
    expect(await screen.findByText('"Show file extensions" is off again.')).toBeTruthy();
  });

  it('explains a rolled back apply: nothing changed', async () => {
    const user = userEvent.setup();
    ipc.listTweaks.mockResolvedValue([fileExtensions]);
    ipc.applyTweak.mockResolvedValue({
      tweak: fileExtensions.id,
      status: 'rolledBack',
      detail: 'the change failed (access denied)',
      detailPhrase: null,
      state: { kind: 'notApplied' },
    } satisfies ApplyResult);
    renderScreen();

    const toggle = await theSwitch();
    await user.click(toggle);
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('Could not turn on "Show file extensions"');
    expect(alert.textContent).toContain('put everything back as it was');
    expect(alert.textContent).toContain('access denied');
    expect(toggle.getAttribute('aria-checked')).toBe('false');
    expect(screen.queryByRole('button', { name: 'Undo' })).toBeNull();

    await user.click(within(alert).getByRole('button', { name: 'Dismiss' }));
    expect(screen.queryByRole('alert')).toBeNull();
  });

  it('explains a needs attention apply and offers Undo for the partial state', async () => {
    const user = userEvent.setup();
    ipc.listTweaks.mockResolvedValue([fileExtensions]);
    ipc.applyTweak.mockResolvedValue({
      tweak: fileExtensions.id,
      status: 'needsAttention',
      detail: '1 change(s) could not be put back automatically',
      detailPhrase: null,
      state: { kind: 'partial' },
    } satisfies ApplyResult);
    ipc.revertTweak.mockResolvedValue(reverted());
    renderScreen();

    await user.click(await theSwitch());
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('only partly on');
    expect(alert.textContent).toContain('could not confirm');
    expect(screen.getByText(/Only partly on\./)).toBeTruthy();

    await user.click(screen.getByRole('button', { name: 'Undo Show file extensions' }));
    expect(ipc.revertTweak).toHaveBeenCalledWith('show-file-extensions');
    await vi.waitFor(() => expect(screen.queryByText(/Only partly on\./)).toBeNull());
  });

  it('shows a command that failed outright as a plain sentence and leaves the state alone', async () => {
    const user = userEvent.setup();
    ipc.listTweaks.mockResolvedValue([fileExtensions]);
    ipc.applyTweak.mockRejectedValue(
      new IpcCommandError({ kind: 'accessDenied', message: 'the store is read only' }),
    );
    renderScreen();

    const toggle = await theSwitch();
    await user.click(toggle);
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('Could not turn on "Show file extensions"');
    expect(alert.textContent).toContain('Access denied: the store is read only');
    expect(toggle.getAttribute('aria-checked')).toBe('false');
    expect(toggle.hasAttribute('disabled')).toBe(false);
  });

  it('disables the switch and says why when the state could not be read', async () => {
    ipc.listTweaks.mockResolvedValue([
      { ...fileExtensions, state: { kind: 'unreadable', message: 'access denied' } },
    ]);
    renderScreen();
    const toggle = await theSwitch();
    expect(toggle.hasAttribute('disabled')).toBe(true);
    expect(screen.getByText(/Could not read the current setting/)).toBeTruthy();
  });
});

describe('search and the Advanced confirmation', () => {
  it('filters the list as the user types and says when nothing matches', async () => {
    advancedMode();
    const user = userEvent.setup();
    ipc.listTweaks.mockResolvedValue([fileExtensions, advanced]);
    renderScreen();
    const list = await screen.findByRole('list', { name: 'Appearance tweaks' });
    expect(within(list).getAllByRole('listitem')).toHaveLength(2);

    await user.type(screen.getByRole('searchbox', { name: 'Search tweaks' }), 'sysmain');
    expect(within(list).getAllByRole('listitem')).toHaveLength(1);

    await user.clear(screen.getByRole('searchbox', { name: 'Search tweaks' }));
    await user.type(screen.getByRole('searchbox', { name: 'Search tweaks' }), 'zzzz');
    expect(screen.queryByRole('list', { name: 'Appearance tweaks' })).toBeNull();
    expect(screen.getByText(/No tweaks match/)).toBeTruthy();
  });

  it('an Advanced tweak asks first, and Cancel applies nothing and returns focus to the switch', async () => {
    advancedMode();
    const user = userEvent.setup();
    ipc.listTweaks.mockResolvedValue([advanced]);
    ipc.applyTweak.mockResolvedValue({ ...applied(), tweak: 'sysmain' });
    renderScreen();

    const toggle = await theSwitch('Stop the SysMain service');
    toggle.focus();
    await user.keyboard(' ');
    expect(await screen.findByRole('dialog', { name: 'Turn on an Advanced tweak?' })).toBeTruthy();
    expect(ipc.getTweakPlan).toHaveBeenCalledWith('sysmain');
    expect(ipc.applyTweak).not.toHaveBeenCalled();

    await user.keyboard('{Escape}');
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(ipc.applyTweak).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(toggle);

    await user.keyboard(' ');
    await screen.findByText('HKCU\\Software\\X\\Flag');
    await user.click(screen.getByRole('button', { name: 'Turn on' }));
    expect(ipc.applyTweak).toHaveBeenCalledWith('sysmain');
    await vi.waitFor(() => expect(toggle.getAttribute('aria-checked')).toBe('true'));
  });
});
