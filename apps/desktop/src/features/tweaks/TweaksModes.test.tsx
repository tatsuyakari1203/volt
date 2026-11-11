import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type {
  PhraseView,
  RevertResult,
  TweakInfo,
  TweakPlanView,
  TweakStepView,
} from '../../shared/ipc';
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

const flagKey = 'HKCU\\Software\\X\\Flag';

const fileExtensions: TweakInfo = {
  id: 'show-file-extensions',
  title: 'Show file extensions',
  description: 'Always show file endings such as .exe and .pdf in File Explorer.',
  category: 'appearance',
  risk: 'safe',
  evidence: 'anecdotal',
  evidenceNote: 'A usability and safety preference.',
  restart: 'none',
  needsAdmin: false,
  state: { kind: 'notApplied' },
  appliedByVolt: false,
};
const caution: TweakInfo = {
  ...fileExtensions,
  id: 'cautious',
  title: 'Cautious one',
  risk: 'caution',
};
const advanced: TweakInfo = {
  ...fileExtensions,
  id: 'sysmain',
  title: 'Stop the SysMain service',
  risk: 'advanced',
};
const gaming: TweakInfo = {
  ...fileExtensions,
  id: 'game-mode',
  title: 'Game mode',
  category: 'gaming',
};
const machine: TweakInfo = {
  ...fileExtensions,
  id: 'machine',
  title: 'Machine wide',
  needsAdmin: true,
};

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
    steps: [step(flagKey, '(not set)', 'dword:0', false)],
    reset: null,
    ...over,
  };
}

const reverted: RevertResult = {
  tweak: 'show-file-extensions',
  status: 'reverted',
  restored: 1,
  failed: 0,
  detail: null,
  detailPhrase: null,
  state: { kind: 'notApplied' },
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
  ipc.getTweakPlan.mockResolvedValue(plan());
});

describe('simple and advanced modes', () => {
  it('shows only Safe tweaks, grouped by category, in Simple mode by default', async () => {
    ipc.listTweaks.mockResolvedValue([fileExtensions, gaming, caution, advanced, machine]);
    renderScreen();
    await screen.findByRole('switch', { name: 'Show file extensions' });
    expect(screen.getByRole('list', { name: 'Appearance tweaks' })).toBeTruthy();
    expect(screen.getByRole('list', { name: 'Gaming tweaks' })).toBeTruthy();
    expect(screen.queryByRole('switch', { name: 'Cautious one' })).toBeNull();
    expect(screen.queryByRole('switch', { name: 'Stop the SysMain service' })).toBeNull();
    expect(screen.queryByRole('switch', { name: 'Machine wide' })).toBeNull();
    expect(screen.getByText(/3 more tweaks are in Advanced mode/)).toBeTruthy();
    expect(screen.queryByText(/Evidence:/)).toBeNull();
    expect(screen.queryByRole('button', { name: /What .* changes/ })).toBeNull();
  });

  it('shows every tweak with its details in Advanced mode and remembers the choice', async () => {
    const user = userEvent.setup();
    ipc.listTweaks.mockResolvedValue([fileExtensions, caution, advanced, machine]);
    const first = renderScreen();
    await screen.findByRole('switch', { name: 'Show file extensions' });
    await user.click(screen.getByRole('radio', { name: 'Advanced' }));
    expect(screen.getByRole('switch', { name: 'Cautious one' })).toBeTruthy();
    expect(screen.getByRole('switch', { name: 'Machine wide' })).toBeTruthy();
    expect(screen.getByText('Caution')).toBeTruthy();
    expect(screen.getByText(/Needs administrator rights/)).toBeTruthy();
    expect(screen.getAllByText(/Evidence:/)).toHaveLength(4);
    expect(window.localStorage.getItem(MODE_STORAGE_KEY)).toBe('advanced');
    first.unmount();

    renderScreen();
    await screen.findByRole('switch', { name: 'Cautious one' });
    const radio = screen.getByRole('radio', { name: 'Advanced' }) as HTMLInputElement;
    expect(radio.checked).toBe(true);
  });

  it('lists exactly what a tweak changes, read through the plan command', async () => {
    window.localStorage.setItem(MODE_STORAGE_KEY, 'advanced');
    const user = userEvent.setup();
    ipc.listTweaks.mockResolvedValue([advanced]);
    ipc.getTweakPlan.mockResolvedValue(
      plan({
        id: 'sysmain',
        steps: [step('service SysMain start type', 'automatic', 'disabled', false)],
      }),
    );
    renderScreen();
    await user.click(
      await screen.findByRole('button', { name: 'What Stop the SysMain service changes' }),
    );
    const steps = await screen.findByRole('list', { name: 'Changes of Stop the SysMain service' });
    expect(ipc.getTweakPlan).toHaveBeenCalledWith('sysmain');
    expect(steps.textContent).toContain('service SysMain start type');
    expect(steps.textContent).toContain('automatic');
    expect(steps.textContent).toContain('disabled');
  });

  it('filters by category and by search', async () => {
    const user = userEvent.setup();
    ipc.listTweaks.mockResolvedValue([fileExtensions, gaming]);
    renderScreen();
    await screen.findByRole('switch', { name: 'Show file extensions' });
    await user.click(screen.getByRole('radio', { name: 'Gaming' }));
    expect(screen.queryByRole('switch', { name: 'Show file extensions' })).toBeNull();
    expect(screen.getByRole('switch', { name: 'Game mode' })).toBeTruthy();
    await user.click(screen.getByRole('radio', { name: 'All' }));
    await user.type(screen.getByRole('searchbox', { name: 'Search tweaks' }), 'extensions');
    expect(screen.queryByRole('switch', { name: 'Game mode' })).toBeNull();
    expect(screen.getByRole('switch', { name: 'Show file extensions' })).toBeTruthy();
  });
});

describe('a tweak that was already on', () => {
  const external: TweakInfo = { ...fileExtensions, state: { kind: 'applied' } };

  it('is labelled as not applied by Volt', async () => {
    ipc.listTweaks.mockResolvedValue([external]);
    renderScreen();
    await screen.findByRole('switch', { name: 'Show file extensions' });
    expect(screen.getByText(/Volt did not turn it on, so it has no earlier value/)).toBeTruthy();
  });

  it('shows what Volt will write instead and resets only after confirming', async () => {
    const user = userEvent.setup();
    ipc.listTweaks.mockResolvedValue([external]);
    ipc.getTweakPlan.mockResolvedValue(
      plan({
        reset: [step(flagKey, 'dword:0', '(not set)', true)],
      }),
    );
    ipc.resetTweak.mockResolvedValue(reverted);
    renderScreen();

    const toggle = await screen.findByRole('switch', { name: 'Show file extensions' });
    await user.click(toggle);
    const dialog = await screen.findByRole('dialog', { name: 'Turn this off?' });
    expect(dialog.textContent).toContain('no earlier value to restore');
    expect(await within(dialog).findByText('Volt will write instead')).toBeTruthy();
    expect(dialog.textContent).toContain('(not set)');
    expect(ipc.resetTweak).not.toHaveBeenCalled();
    expect(ipc.revertTweak).not.toHaveBeenCalled();

    await user.click(within(dialog).getByRole('button', { name: 'Turn off' }));
    expect(ipc.resetTweak).toHaveBeenCalledWith('show-file-extensions');
    await vi.waitFor(() => expect(toggle.getAttribute('aria-checked')).toBe('false'));
    expect(await screen.findByText(/Windows default/)).toBeTruthy();
  });

  it('cannot be switched off when Volt does not know the default', async () => {
    const user = userEvent.setup();
    ipc.listTweaks.mockResolvedValue([external]);
    ipc.getTweakPlan.mockResolvedValue(plan({ reset: null }));
    renderScreen();
    await user.click(await screen.findByRole('switch', { name: 'Show file extensions' }));
    const dialog = await screen.findByRole('dialog', { name: 'Turn this off?' });
    expect(await within(dialog).findByText(/Volt cannot switch this off safely/)).toBeTruthy();
    const button = within(dialog).getByRole('button', { name: 'Turn off' }) as HTMLButtonElement;
    expect(button.disabled).toBe(true);
  });
});

describe('confirming a Caution tweak', () => {
  it('shows the plan before applying anything above Safe', async () => {
    window.localStorage.setItem(MODE_STORAGE_KEY, 'advanced');
    const user = userEvent.setup();
    ipc.listTweaks.mockResolvedValue([caution]);
    ipc.applyTweak.mockResolvedValue({
      tweak: 'cautious',
      status: 'applied',
      detail: null,
      detailPhrase: null,
      state: { kind: 'applied' },
    });
    renderScreen();
    await user.click(await screen.findByRole('switch', { name: 'Cautious one' }));
    const dialog = await screen.findByRole('dialog', { name: 'Turn on a Caution tweak?' });
    expect(await within(dialog).findByText('It will change')).toBeTruthy();
    expect(ipc.applyTweak).not.toHaveBeenCalled();
    await user.click(within(dialog).getByRole('button', { name: 'Turn on' }));
    expect(ipc.applyTweak).toHaveBeenCalledWith('cautious');
  });
});
