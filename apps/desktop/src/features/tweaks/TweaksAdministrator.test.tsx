import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type {
  ApplyResult,
  ElevationStatusView,
  RevertResult,
  TweakInfo,
  TweakPlanView,
} from '../../shared/ipc';
import { ToastProvider } from '../../shared/ui';
import { MODE_STORAGE_KEY, TweaksScreen } from './TweaksScreen';

const ipc = vi.hoisted(() => ({
  listTweaks: vi.fn(),
  applyTweak: vi.fn(),
  revertTweak: vi.fn(),
  getTweakPlan: vi.fn(),
  resetTweak: vi.fn(),
  getElevationStatus: vi.fn(),
  getSystemProtection: vi.fn(),
}));
vi.mock('../../shared/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../shared/ipc')>()),
  ...ipc,
}));

const machine: TweakInfo = {
  id: 'turn-off-error-reporting',
  title: 'Turn off Windows Error Reporting',
  description: 'Windows stops offering to send crash reports to Microsoft.',
  category: 'privacy',
  risk: 'safe',
  evidence: 'anecdotal',
  evidenceNote: 'A privacy choice.',
  restart: 'none',
  needsAdmin: true,
  state: { kind: 'notApplied' },
  appliedByVolt: false,
};

const available: ElevationStatusView = { state: 'available' };

function applyResult(status: ApplyResult['status'], applied = false): ApplyResult {
  return {
    tweak: machine.id,
    status,
    detail: null,
    detailPhrase: null,
    state: { kind: applied ? 'applied' : 'notApplied' },
  };
}

function revertResult(status: RevertResult['status']): RevertResult {
  return {
    tweak: machine.id,
    status,
    restored: status === 'reverted' ? 1 : 0,
    failed: 0,
    detail: null,
    detailPhrase: null,
    state: { kind: status === 'reverted' ? 'notApplied' : 'applied' },
  };
}

function plan(): TweakPlanView {
  return { id: machine.id, steps: [], reset: null };
}

function renderScreen() {
  return render(
    <ToastProvider>
      <TweaksScreen />
    </ToastProvider>,
  );
}

async function theSwitch() {
  return (await screen.findByRole('switch', {
    name: 'Turn off Windows Error Reporting',
  })) as HTMLButtonElement;
}

beforeEach(() => {
  for (const fn of Object.values(ipc)) fn.mockReset();
  window.localStorage.setItem(MODE_STORAGE_KEY, 'advanced');
  ipc.getTweakPlan.mockResolvedValue(plan());
  ipc.listTweaks.mockResolvedValue([machine]);
  ipc.getElevationStatus.mockResolvedValue(available);
  ipc.getSystemProtection.mockResolvedValue('unknown');
});

describe('administrator tweaks through the helper', () => {
  it('turns one on through the shield switch, with the administrator badge and the hint', async () => {
    const user = userEvent.setup();
    ipc.applyTweak.mockResolvedValue(applyResult('applied', true));
    renderScreen();
    const row = (await screen.findByRole('listitem')) as HTMLElement;
    expect(within(row).getByText('Administrator')).toBeTruthy();
    expect(row.textContent).toContain('Windows asks you to approve once per session');
    const control = await theSwitch();
    expect(control.disabled).toBe(false);
    await user.click(control);
    expect(ipc.applyTweak).toHaveBeenCalledWith(machine.id);
    expect(await screen.findByText(/is on\./)).toBeTruthy();
    expect((await theSwitch()).getAttribute('aria-checked')).toBe('true');
  });

  it('says why when administrator actions are not available, and keeps the switch off', async () => {
    ipc.getElevationStatus.mockResolvedValue({
      state: 'unavailable',
      reason: 'notAllUsers',
      detail: 'x',
    });
    renderScreen();
    const control = await theSwitch();
    expect(control.disabled).toBe(true);
    expect(await screen.findByText(/Install Volt for all users/)).toBeTruthy();
    expect(ipc.applyTweak).not.toHaveBeenCalled();
  });

  it('is locked, with the plain label, while the availability is not known', async () => {
    ipc.getElevationStatus.mockRejectedValue(new Error('not in the app'));
    renderScreen();
    expect((await theSwitch()).disabled).toBe(true);
    expect(screen.getByText(/Needs administrator rights/)).toBeTruthy();
  });

  it('shows what happened when nobody confirmed, in words, and changes nothing on screen', async () => {
    const user = userEvent.setup();
    renderScreen();
    for (const [status, text] of [
      ['notConfirmed', /confirmation was declined or timed out/],
      ['blocked', /Try again in 10 minutes/],
      ['restorePointFailed', /Turn on System Protection for the system drive/],
      ['timedOut', /stopped waiting for "Turn off Windows Error Reporting"/],
      ['declined', /cancelled the administrator prompt/],
    ] as const) {
      ipc.applyTweak.mockResolvedValueOnce(applyResult(status));
      await user.click(await theSwitch());
      const alert = await screen.findByRole('alert');
      expect(alert.textContent).toMatch(text);
      expect((await theSwitch()).getAttribute('aria-checked')).toBe('false');
    }
  });

  it('says before the user tries that System Protection is off, for a tweak that needs a restore point', async () => {
    ipc.getSystemProtection.mockResolvedValue('off');
    ipc.listTweaks.mockResolvedValue([
      { ...machine, risk: 'caution' },
      { ...machine, id: 'safe-one', title: 'A safe one', risk: 'safe' },
    ]);
    renderScreen();
    const rows = (await screen.findAllByRole('listitem')) as HTMLElement[];
    expect(rows).toHaveLength(2);
    const caution = rows[0] as HTMLElement;
    const safe = rows[1] as HTMLElement;
    expect(caution.textContent).toContain('System Protection is off');
    expect(caution.textContent).toContain('System Protection tab');
    // The switch is not locked by the hint: the helper is the authority.
    expect((within(caution).getByRole('switch') as HTMLButtonElement).disabled).toBe(false);
    // A Safe tweak needs no restore point, so it says nothing about it.
    expect(safe.textContent).not.toContain('System Protection is off');
  });

  it('turns one off through the helper only when Volt turned it on', async () => {
    const user = userEvent.setup();
    ipc.listTweaks.mockResolvedValue([
      { ...machine, state: { kind: 'applied' }, appliedByVolt: true },
    ]);
    ipc.revertTweak.mockResolvedValueOnce(revertResult('declined'));
    renderScreen();
    await user.click(await theSwitch());
    expect(ipc.revertTweak).toHaveBeenCalledWith(machine.id);
    expect((await screen.findByRole('alert')).textContent).toContain('was not turned off');
    // The refusal changed nothing: it is still Volt's to undo.
    expect((await theSwitch()).disabled).toBe(false);
    ipc.revertTweak.mockResolvedValueOnce(revertResult('reverted'));
    await user.click(await theSwitch());
    expect(await screen.findByText(/is off again/)).toBeTruthy();
  });

  it('leaves a tweak that was on before Volt alone', async () => {
    ipc.listTweaks.mockResolvedValue([
      { ...machine, state: { kind: 'applied' }, appliedByVolt: false },
    ]);
    renderScreen();
    expect((await theSwitch()).disabled).toBe(true);
    expect(screen.getByText(/Already on, but Volt did not turn it on/)).toBeTruthy();
  });
});
