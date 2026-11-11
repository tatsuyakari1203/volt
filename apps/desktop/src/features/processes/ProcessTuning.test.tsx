import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type {
  InspectionView,
  ProcessGroupView,
  ProcessListView,
  RuleView,
  TuneResult,
} from '../../shared/ipc';
import { ToastProvider } from '../../shared/ui';
import { ProcessesScreen } from './ProcessesScreen';
import { buildRequest } from './TuneDialog';

const ipc = vi.hoisted(() => ({
  listProcesses: vi.fn(),
  endApp: vi.fn(),
  listProcessRules: vi.fn(),
  removeProcessRule: vi.fn(),
  inspectApp: vi.fn(),
  tuneApp: vi.fn(),
}));
vi.mock('../../shared/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../shared/ipc')>()),
  ...ipc,
}));

const MB = 1024 ** 2;

function group(id: string, over: Partial<ProcessGroupView> = {}): ProcessGroupView {
  return {
    id,
    name: id,
    path: null,
    processCount: 3,
    endCount: 3,
    memoryBytes: 100 * MB,
    cpuPercent: 0,
    endability: 'allowed',
    ...over,
  };
}

const sample: ProcessListView = {
  groups: [
    group('game.exe'),
    group('svchost.exe', { endability: 'protectedWindows', endCount: 0 }),
  ],
  processCount: 6,
  logicalCpus: 4,
};

const inspection: InspectionView = {
  name: 'game.exe',
  path: null,
  processes: 3,
  changeable: 2,
  needsAdministrator: 1,
  unsupported: 0,
  priority: 'normal',
  cores: [0, 1, 2, 3],
  availableCores: [0, 1, 2, 3],
  efficiency: false,
};

const done: TuneResult = {
  name: 'game.exe',
  changed: 2,
  alreadySet: 0,
  needsAdministrator: 0,
  skipped: 0,
  recorded: true,
  ruleSaved: false,
};

function renderScreen() {
  return render(
    <ToastProvider>
      <ProcessesScreen />
    </ToastProvider>,
  );
}

async function openSettings(user: ReturnType<typeof userEvent.setup>) {
  await screen.findByRole('list', { name: 'Apps' });
  await user.click(screen.getByRole('button', { name: 'Settings for game.exe' }));
  return screen.findByRole('dialog', { name: 'Settings for game.exe' });
}

beforeEach(() => {
  for (const mock of Object.values(ipc)) mock.mockReset();
  ipc.listProcesses.mockResolvedValue(sample);
  ipc.listProcessRules.mockResolvedValue([]);
  ipc.inspectApp.mockResolvedValue(inspection);
  ipc.tuneApp.mockResolvedValue(done);
});

describe('buildRequest', () => {
  it('leaves what is on "leave as it is" alone and sorts the cores', () => {
    expect(
      buildRequest({ priority: 'keep', limitCores: false, cores: [3, 1], efficiency: 'keep' }),
    ).toEqual({ priority: null, cores: null, efficiency: null });
    expect(
      buildRequest({ priority: 'high', limitCores: true, cores: [3, 1], efficiency: 'off' }),
    ).toEqual({ priority: 'high', cores: [1, 3], efficiency: false });
  });

  it('does not send a priority with Efficiency mode, which sets Low itself', () => {
    expect(
      buildRequest({ priority: 'high', limitCores: false, cores: [0], efficiency: 'on' }),
    ).toEqual({ priority: null, cores: null, efficiency: true });
  });
});

describe('the Settings dialog of an app', () => {
  it('has no Settings button for a protected app', async () => {
    renderScreen();
    await screen.findByRole('list', { name: 'Apps' });
    expect(screen.getByRole('button', { name: 'Settings for game.exe' })).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Settings for svchost.exe' })).toBeNull();
  });

  it('reads what the processes hold, says how many can be changed and why not all', async () => {
    const user = userEvent.setup();
    renderScreen();
    const dialog = await openSettings(user);
    expect(ipc.inspectApp).toHaveBeenCalledWith('game.exe');
    expect(dialog.textContent).toContain('2 of 3 processes can be changed.');
    expect(dialog.textContent).toContain('1 process need administrator rights');
    expect(dialog.textContent).toContain('Now: Normal');
    // Apply waits for a choice.
    expect(within(dialog).getByRole('button', { name: 'Apply' }).hasAttribute('disabled')).toBe(
      true,
    );
  });

  it('offers the five priority classes and never Realtime', async () => {
    const user = userEvent.setup();
    renderScreen();
    const dialog = await openSettings(user);
    const select = within(dialog).getByRole('combobox', { name: /^Priority/ });
    const options = within(select).getAllByRole('option');
    expect(options.map((o) => o.textContent)).toEqual([
      'Leave as it is',
      'Low',
      'Below normal',
      'Normal',
      'Above normal',
      'High',
    ]);
    expect(dialog.textContent?.toLowerCase()).toContain('realtime is not offered');
  });

  it('applies the chosen priority, cores and Efficiency mode, and says what it did', async () => {
    const user = userEvent.setup();
    renderScreen();
    const dialog = await openSettings(user);
    await user.selectOptions(within(dialog).getByRole('combobox', { name: /^Priority/ }), 'high');
    await user.click(within(dialog).getByRole('checkbox', { name: 'Run only on chosen cores' }));
    await user.click(within(dialog).getByRole('checkbox', { name: 'Core 3' }));
    await user.selectOptions(
      within(dialog).getByRole('combobox', { name: /^Efficiency mode/ }),
      'off',
    );
    await user.click(within(dialog).getByRole('button', { name: 'Apply' }));

    expect(ipc.tuneApp).toHaveBeenCalledWith(
      'game.exe',
      { priority: 'high', cores: [0, 1, 2], efficiency: false },
      false,
    );
    expect(await screen.findByText(/Changed game\.exe: 2 processes\./)).toBeTruthy();
    expect(screen.queryByRole('dialog')).toBeNull();
  });

  it('keeps the last core chosen, so an empty set is never sent', async () => {
    const user = userEvent.setup();
    ipc.inspectApp.mockResolvedValue({ ...inspection, cores: [2], availableCores: [0, 1, 2, 3] });
    renderScreen();
    const dialog = await openSettings(user);
    await user.click(within(dialog).getByRole('checkbox', { name: 'Run only on chosen cores' }));
    const core2 = within(dialog).getByRole('checkbox', { name: 'Core 2' }) as HTMLInputElement;
    expect(core2.checked).toBe(true);
    await user.click(core2);
    expect(core2.checked).toBe(true);
    // Choosing another one lets the first go.
    await user.click(within(dialog).getByRole('checkbox', { name: 'Core 0' }));
    await user.click(core2);
    expect(core2.checked).toBe(false);
    await user.click(within(dialog).getByRole('button', { name: 'Apply' }));
    expect(ipc.tuneApp).toHaveBeenCalledWith(
      'game.exe',
      { priority: null, cores: [0], efficiency: null },
      false,
    );
  });

  it('Efficiency mode takes the priority over: Low is shown and no priority is sent', async () => {
    const user = userEvent.setup();
    renderScreen();
    const dialog = await openSettings(user);
    await user.selectOptions(within(dialog).getByRole('combobox', { name: /^Priority/ }), 'high');
    await user.selectOptions(
      within(dialog).getByRole('combobox', { name: /^Efficiency mode/ }),
      'on',
    );
    const priority = within(dialog).getByRole('combobox', {
      name: /^Priority/,
    }) as HTMLSelectElement;
    expect(priority.disabled).toBe(true);
    expect(priority.value).toBe('idle');
    expect(dialog.textContent).toContain('Efficiency mode sets Low priority.');
    await user.click(within(dialog).getByRole('button', { name: 'Apply' }));
    expect(ipc.tuneApp).toHaveBeenCalledWith(
      'game.exe',
      { priority: null, cores: null, efficiency: true },
      false,
    );
  });

  it('saves the rule when asked and lists it afterwards', async () => {
    const user = userEvent.setup();
    const rule: RuleView = {
      id: 'game.exe',
      name: 'game.exe',
      path: null,
      request: { priority: 'belowNormal', cores: [0, 1], efficiency: null },
      problem: null,
      leftAlone: 0,
    };
    ipc.tuneApp.mockResolvedValue({ ...done, ruleSaved: true });
    renderScreen();
    const dialog = await openSettings(user);
    await user.selectOptions(
      within(dialog).getByRole('combobox', { name: /^Priority/ }),
      'belowNormal',
    );
    await user.click(
      within(dialog).getByRole('checkbox', {
        name: /Apply these settings again whenever game\.exe starts/,
      }),
    );
    ipc.listProcessRules.mockResolvedValue([rule]);
    await user.click(within(dialog).getByRole('button', { name: 'Apply' }));

    expect(ipc.tuneApp).toHaveBeenCalledWith(
      'game.exe',
      { priority: 'belowNormal', cores: null, efficiency: null },
      true,
    );
    expect(await screen.findByText(/Saved as a rule/)).toBeTruthy();
    const rules = await screen.findByRole('list', { name: 'Saved rules' });
    expect(within(rules).getByText('Below normal priority, 2 cores')).toBeTruthy();
  });

  it('says why nothing can be changed when Windows needs an administrator', async () => {
    const user = userEvent.setup();
    ipc.inspectApp.mockResolvedValue({
      ...inspection,
      changeable: 0,
      needsAdministrator: 3,
      priority: null,
      cores: null,
      availableCores: [],
      efficiency: null,
    });
    renderScreen();
    const dialog = await openSettings(user);
    expect(dialog.textContent).toContain('Windows lets only an administrator change game.exe');
    expect(within(dialog).queryByRole('combobox')).toBeNull();
    expect(within(dialog).getByRole('button', { name: 'Apply' }).hasAttribute('disabled')).toBe(
      true,
    );
  });

  it('reports a refusal and keeps the dialog open', async () => {
    const user = userEvent.setup();
    ipc.tuneApp.mockRejectedValue(new Error('at least one core must stay selected'));
    renderScreen();
    const dialog = await openSettings(user);
    await user.selectOptions(within(dialog).getByRole('combobox', { name: /^Priority/ }), 'idle');
    await user.click(within(dialog).getByRole('button', { name: 'Apply' }));
    expect((await within(dialog).findByRole('alert')).textContent).toContain(
      'at least one core must stay selected',
    );
    expect(screen.getByRole('dialog', { name: 'Settings for game.exe' })).toBeTruthy();
  });

  it('flags processes that were left alone in the notice', async () => {
    const user = userEvent.setup();
    ipc.tuneApp.mockResolvedValue({ ...done, changed: 1, needsAdministrator: 1 });
    renderScreen();
    const dialog = await openSettings(user);
    await user.selectOptions(within(dialog).getByRole('combobox', { name: /^Priority/ }), 'idle');
    await user.click(within(dialog).getByRole('button', { name: 'Apply' }));
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('1 process need administrator rights and were left alone');
  });
});

describe('the saved rules', () => {
  const rules: RuleView[] = [
    {
      id: 'c:\\games\\game.exe',
      name: 'game.exe',
      path: 'c:\\games\\game.exe',
      request: { priority: null, cores: null, efficiency: true },
      problem: null,
      leftAlone: 2,
    },
    {
      id: 'vendor.exe',
      name: 'vendor.exe',
      path: null,
      request: { priority: 'high', cores: [0], efficiency: null },
      problem: 'needsAdministrator',
      leftAlone: 0,
    },
  ];

  it('are listed with what they apply and the problem of one that could not be applied', async () => {
    ipc.listProcessRules.mockResolvedValue(rules);
    renderScreen();
    const list = await screen.findByRole('list', { name: 'Saved rules' });
    const items = within(list).getAllByRole('listitem');
    expect(items).toHaveLength(2);
    // Efficiency mode on is Low priority: it is said once.
    expect(items[0]?.textContent).toContain('Efficiency mode on');
    expect(items[0]?.textContent).not.toContain('priority');
    expect(items[0]?.textContent).toContain('2 processes could not be changed');
    expect(items[1]?.textContent).toContain('High priority, 1 core');
    expect(items[1]?.textContent).toContain('Not applied: Windows lets only an administrator');
  });

  it('say so when there are none', async () => {
    renderScreen();
    expect(await screen.findByText(/No rules yet/)).toBeTruthy();
  });

  it('can be removed, and the list shows what is left', async () => {
    const user = userEvent.setup();
    ipc.listProcessRules.mockResolvedValue(rules);
    ipc.removeProcessRule.mockResolvedValue([rules[1]]);
    renderScreen();
    await screen.findByRole('list', { name: 'Saved rules' });
    await user.click(screen.getByRole('button', { name: 'Remove the rule for game.exe' }));
    expect(ipc.removeProcessRule).toHaveBeenCalledWith('c:\\games\\game.exe');
    const list = screen.getByRole('list', { name: 'Saved rules' });
    expect(within(list).getAllByRole('listitem')).toHaveLength(1);
    expect(screen.queryByRole('button', { name: 'Remove the rule for game.exe' })).toBeNull();
  });

  it('report a reading that failed without hiding the screen', async () => {
    ipc.listProcessRules.mockRejectedValue(new Error('store busy'));
    renderScreen();
    expect(await screen.findByText(/The saved rules could not be read: store busy/)).toBeTruthy();
    expect(await screen.findByRole('list', { name: 'Apps' })).toBeTruthy();
  });
});
