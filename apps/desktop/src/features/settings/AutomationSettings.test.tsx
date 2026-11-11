import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';
import type { AutomationStatusView, CleaningRuleInfo } from '../../shared/ipc';
import { AutomationSettings } from './AutomationSettings';

const ipc = vi.hoisted(() => ({
  getAutomationStatus: vi.fn(),
  setAutomationConfig: vi.fn(),
  onAutomationUpdated: vi.fn(),
  listCleaningRules: vi.fn(),
}));
vi.mock('../../shared/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../shared/ipc')>()),
  ...ipc,
}));

function rule(id: string, title: string, over: Partial<CleaningRuleInfo> = {}): CleaningRuleInfo {
  return {
    id,
    title,
    description: '',
    category: 'temporaryFiles',
    categoryLabel: 'Temporary files',
    risk: 'safe',
    needsAdministrator: false,
    minAgeDays: 7,
    pickItems: false,
    needsFolder: false,
    chosenFolder: null,
    follows: [],
    ...over,
  };
}

const rules = [
  rule('user-temp', 'Temporary files'),
  rule('windows-temp', 'Windows temporary files', { needsAdministrator: true }),
  rule('browser-cache', 'Browser caches', { pickItems: true }),
  rule('comfy', 'Image tool cache', { needsFolder: true }),
  rule('thumbnails', 'Thumbnail cache'),
];

const off: AutomationStatusView = {
  config: {
    memory: { enabled: false, standbyAboveMb: 4096, freeBelowMb: 1024, cooldownMinutes: 30 },
    cleaning: {
      enabled: false,
      frequency: 'daily',
      weekday: 6,
      minuteOfDay: 180,
      ruleIds: [],
    },
    notify: true,
  },
  bounds: { standbyAboveMb: [256, 262144], freeBelowMb: [128, 131072], cooldownMinutes: [5, 1440] },
  lastMemoryRun: null,
  lastCleanRun: null,
  lastCleanSkip: null,
  nextCleanRun: null,
  paused: null,
};

beforeEach(() => {
  for (const mock of Object.values(ipc)) mock.mockReset();
  ipc.getAutomationStatus.mockResolvedValue(off);
  ipc.listCleaningRules.mockResolvedValue(rules);
  ipc.onAutomationUpdated.mockResolvedValue(() => {});
});

it('shows both features off, says what each will do and offers only rules that can run alone', async () => {
  render(<AutomationSettings />);
  const memory = await screen.findByRole('switch', { name: 'Clean memory when it runs low' });
  await vi.waitFor(() => expect(memory.hasAttribute('disabled')).toBe(false));
  expect(memory.getAttribute('aria-checked')).toBe('false');
  expect(
    screen.getByRole('switch', { name: 'Clean on a schedule' }).getAttribute('aria-checked'),
  ).toBe('false');
  expect(
    screen.getByText(/standby memory is above 4,096 MB and free memory is below 1,024 MB/),
  ).toBeTruthy();
  expect(screen.getByText(/Every day at 03:00/)).toBeTruthy();
  // The administrator rule, the picked rule and the chosen-folder rule cannot be scheduled.
  const list = within(screen.getByRole('list', { name: 'Rules to clean' }));
  expect(list.getByRole('switch', { name: 'Temporary files' })).toBeTruthy();
  expect(list.getByRole('switch', { name: 'Thumbnail cache' })).toBeTruthy();
  expect(list.queryByRole('switch', { name: 'Windows temporary files' })).toBeNull();
  expect(list.queryByRole('switch', { name: 'Browser caches' })).toBeNull();
  expect(list.queryByRole('switch', { name: 'Image tool cache' })).toBeNull();
  expect(screen.getByText('Last automatic memory clean: never')).toBeTruthy();
  expect(screen.getByText('Last scheduled clean: never')).toBeTruthy();
  // Notifications follow the existing rule and can be turned off.
  expect(
    screen
      .getByRole('switch', { name: 'Notify me after an automatic run' })
      .getAttribute('aria-checked'),
  ).toBe('true');
});

it('turns the memory rule on through the typed command and shows what the shell stored', async () => {
  const user = userEvent.setup();
  const on = { ...off, config: { ...off.config, memory: { ...off.config.memory, enabled: true } } };
  ipc.setAutomationConfig.mockResolvedValue(on);
  render(<AutomationSettings />);
  const memory = await screen.findByRole('switch', { name: 'Clean memory when it runs low' });
  await vi.waitFor(() => expect(memory.hasAttribute('disabled')).toBe(false));
  await user.click(memory);
  expect(ipc.setAutomationConfig).toHaveBeenCalledWith(on.config);
  await vi.waitFor(() => expect(memory.getAttribute('aria-checked')).toBe('true'));
});

it('keeps a typed number inside the bounds before it is saved', async () => {
  const user = userEvent.setup();
  ipc.setAutomationConfig.mockImplementation(async (config) => ({ ...off, config }));
  render(<AutomationSettings />);
  const cooldown = await screen.findByRole('spinbutton', { name: /Wait between cleans/ });
  await vi.waitFor(() => expect(cooldown.hasAttribute('disabled')).toBe(false));
  await user.clear(cooldown);
  await user.type(cooldown, '99999');
  await user.tab();
  expect(ipc.setAutomationConfig).toHaveBeenCalledTimes(1);
  expect(ipc.setAutomationConfig.mock.calls[0]?.[0].memory.cooldownMinutes).toBe(1440);
});

it('saves the chosen rules and the weekly day, and warns when a schedule has no rule', async () => {
  const user = userEvent.setup();
  const scheduled = {
    ...off,
    config: { ...off.config, cleaning: { ...off.config.cleaning, enabled: true } },
  };
  ipc.getAutomationStatus.mockResolvedValue(scheduled);
  ipc.setAutomationConfig.mockImplementation(async (config) => ({ ...scheduled, config }));
  render(<AutomationSettings />);
  expect(await screen.findByText(/Choose at least one rule/)).toBeTruthy();
  const temp = within(await screen.findByRole('list', { name: 'Rules to clean' })).getByRole(
    'switch',
    { name: 'Temporary files' },
  );
  await vi.waitFor(() => expect(temp.hasAttribute('disabled')).toBe(false));
  await user.click(temp);
  expect(ipc.setAutomationConfig.mock.calls[0]?.[0].cleaning.ruleIds).toEqual(['user-temp']);
  await vi.waitFor(() => expect(screen.queryByText(/Choose at least one rule/)).toBeNull());

  await user.click(screen.getByRole('radio', { name: 'Weekly' }));
  expect(ipc.setAutomationConfig.mock.calls[1]?.[0].cleaning.frequency).toBe('weekly');
  await user.selectOptions(await screen.findByRole('combobox', { name: 'Day' }), '0');
  expect(ipc.setAutomationConfig.mock.calls[2]?.[0].cleaning.weekday).toBe(0);
});

it('says what is paused now and what the last scheduled run did', async () => {
  ipc.getAutomationStatus.mockResolvedValue({
    ...off,
    config: {
      ...off.config,
      cleaning: { ...off.config.cleaning, enabled: true, ruleIds: ['user-temp'] },
    },
    paused: 'fullscreen',
    lastCleanSkip: { at: Date.UTC(2027, 0, 15, 3, 0), reason: 'batterySaver' },
    nextCleanRun: Date.UTC(2027, 0, 16, 3, 0),
  } satisfies AutomationStatusView);
  render(<AutomationSettings />);
  expect(
    await screen.findByText('Paused now: a fullscreen app or a presentation is active.'),
  ).toBeTruthy();
  expect(screen.getByText(/Skipped on .*: battery saver was on\./)).toBeTruthy();
  expect(screen.getByText(/Next scheduled clean:/)).toBeTruthy();
});

it('shows the reason when the settings cannot be read', async () => {
  ipc.getAutomationStatus.mockRejectedValue(new Error('no shell'));
  render(<AutomationSettings />);
  expect(await screen.findByRole('alert')).toBeTruthy();
  expect(
    screen.getByRole('switch', { name: 'Clean memory when it runs low' }).hasAttribute('disabled'),
  ).toBe(true);
});
