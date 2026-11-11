import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';
import type {
  AutomationStatusView,
  ProfilesView,
  SwitchPreviewView,
  TweakInfo,
} from '../../shared/ipc';
import { ProfilesScreen } from './ProfilesScreen';

const ipc = vi.hoisted(() => ({
  listProfiles: vi.fn(),
  listTweaks: vi.fn(),
  getAutomationStatus: vi.fn(),
  onProfilesUpdated: vi.fn(),
  previewProfileSwitch: vi.fn(),
  switchProfile: vi.fn(),
  saveProfile: vi.fn(),
  deleteProfile: vi.fn(),
  undoProfileSwitch: vi.fn(),
}));
vi.mock('../../shared/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../shared/ipc')>()),
  ...ipc,
}));

const memory = { enabled: true, standbyAboveMb: 4096, freeBelowMb: 1024, cooldownMinutes: 30 };

function profile(id: string, name: string, tweaks: string[], over = {}) {
  return {
    id,
    name,
    tweaks: tweaks.map((tweak) => ({ id: tweak, enabled: true })),
    automation: { memory: null, cleaning: null },
    trigger: null,
    ...over,
  };
}

function view(over: Partial<ProfilesView> = {}): ProfilesView {
  return {
    profiles: [
      profile('gaming', 'Gaming', ['faster-menu-display', 'end-hung-apps-faster'], {
        automation: { memory, cleaning: null },
      }),
      profile('work', 'Work', ['show-file-extensions'], {
        trigger: { exe: 'code.exe', returnOnExit: true },
      }),
    ],
    suggested: true,
    active: null,
    lastSwitch: null,
    lastSkip: null,
    watching: true,
    pollSeconds: 15,
    maxProfiles: 24,
    maxNameChars: 40,
    ...over,
  };
}

function tweak(id: string, title: string, over: Partial<TweakInfo> = {}): TweakInfo {
  return {
    id,
    title,
    description: '',
    category: 'performance',
    risk: 'safe',
    evidence: 'anecdotal',
    evidenceNote: '',
    restart: 'none',
    needsAdmin: false,
    state: { kind: 'notApplied' },
    appliedByVolt: false,
    ...over,
  };
}

const tweaks = [
  tweak('faster-menu-display', 'Faster menu display'),
  tweak('end-hung-apps-faster', 'End hung apps faster', { risk: 'caution' }),
  tweak('show-file-extensions', 'Show file extensions'),
  tweak('turn-off-search-indexing', 'Turn off search indexing', { needsAdmin: true }),
];

const step = (subject: string) => ({
  subject,
  current: 'REG_DWORD 1',
  target: 'REG_DWORD 0',
  subjectPhrase: { code: 'x', args: {} },
  currentPhrase: { code: 'x', args: {} },
  targetPhrase: { code: 'x', args: {} },
  atTarget: false,
});

const preview: SwitchPreviewView = {
  from: null,
  to: 'gaming',
  items: [
    {
      tweak: 'faster-menu-display',
      title: 'Faster menu display',
      risk: 'safe',
      action: 'turnOn',
      steps: [step('HKCU\\Control Panel\\Desktop\\MenuShowDelay')],
      restores: [],
    },
    {
      tweak: 'end-hung-apps-faster',
      title: 'End hung apps faster',
      risk: 'caution',
      action: 'turnOn',
      steps: [step('HKCU\\Control Panel\\Desktop\\HungAppTimeout')],
      restores: [],
    },
  ],
  leftOut: [
    {
      tweak: 'turn-off-search-indexing',
      title: 'Turn off search indexing',
      reason: 'needsAdministrator',
      detail: null,
    },
  ],
  inPlace: 1,
  automation: {
    before: {
      memory: { ...memory, enabled: false },
      cleaning: {
        enabled: false,
        frequency: 'daily',
        weekday: 6,
        minuteOfDay: 180,
        ruleIds: [],
      },
      notify: true,
    },
    after: {
      memory,
      cleaning: {
        enabled: false,
        frequency: 'daily',
        weekday: 6,
        minuteOfDay: 180,
        ruleIds: [],
      },
      notify: true,
    },
  },
  paused: null,
};

const automationStatus = {
  config: preview.automation?.before,
} as AutomationStatusView;

beforeEach(() => {
  for (const mock of Object.values(ipc)) mock.mockReset();
  ipc.listProfiles.mockResolvedValue(view());
  ipc.listTweaks.mockResolvedValue(tweaks);
  ipc.getAutomationStatus.mockResolvedValue(automationStatus);
  ipc.onProfilesUpdated.mockResolvedValue(() => {});
});

it('lists the suggestions with none active, and says what watching an app costs', async () => {
  render(<ProfilesScreen />);
  expect(await screen.findByText('No profile is active.')).toBeTruthy();
  expect(screen.getByText(/These are suggestions to start from/)).toBeTruthy();
  const list = screen.getByRole('list', { name: 'Profiles' });
  expect(within(list).getByText('Gaming')).toBeTruthy();
  expect(within(list).getByText(/2 tweaks · Automation: memory rule on/)).toBeTruthy();
  expect(
    within(list).getByText(/Switches on when code\.exe starts, back when it exits/),
  ).toBeTruthy();
  // The cost and the limits are stated where the trigger shows.
  const note = screen.getByRole('region', { name: 'Watching for apps' });
  expect(within(note).getByText(/every 15 seconds from the tray/)).toBeTruthy();
  expect(within(note).getByText(/never made up later/)).toBeTruthy();
});

it('previews a switch exactly, names the Caution tweak, and switches after confirming', async () => {
  const user = userEvent.setup();
  ipc.previewProfileSwitch.mockResolvedValue(preview);
  ipc.switchProfile.mockResolvedValue({
    from: null,
    to: 'gaming',
    steps: preview.items.map((item) => ({
      tweak: item.tweak,
      title: item.title,
      turnedOn: true,
      status: 'done',
      detail: null,
      detailPhrase: null,
    })),
    automation: 'set',
    complete: true,
  });
  render(<ProfilesScreen />);
  await user.click(await screen.findByRole('button', { name: 'Switch to Gaming' }));

  const dialog = await screen.findByRole('dialog', { name: 'Switch to Gaming' });
  expect(ipc.previewProfileSwitch).toHaveBeenCalledWith('gaming');
  await within(dialog).findByText(/Caution: this switch changes End hung apps faster/);
  expect(within(dialog).getByText('HKCU\\Control Panel\\Desktop\\HungAppTimeout')).toBeTruthy();
  expect(within(dialog).getByText('Memory rule: off to on')).toBeTruthy();
  expect(
    within(dialog).getByText(/needs administrator rights, so a profile does not change it/),
  ).toBeTruthy();
  expect(ipc.switchProfile).not.toHaveBeenCalled();

  await user.click(within(dialog).getByRole('button', { name: 'Switch to Gaming' }));
  expect(ipc.switchProfile).toHaveBeenCalledWith('gaming');
  expect(await within(dialog).findByText(/Switched to Gaming\. 2 tweaks changed\./)).toBeTruthy();
  expect(within(dialog).getByText('The automation settings were updated.')).toBeTruthy();
  // The page read the profiles again.
  expect(ipc.listProfiles).toHaveBeenCalledTimes(2);
});

it('says what was and was not changed when a switch stops part-way', async () => {
  const user = userEvent.setup();
  ipc.previewProfileSwitch.mockResolvedValue(preview);
  ipc.switchProfile.mockResolvedValue({
    from: null,
    to: 'gaming',
    steps: [
      {
        tweak: 'faster-menu-display',
        title: 'Faster menu display',
        turnedOn: true,
        status: 'done',
        detail: null,
        detailPhrase: null,
      },
      {
        tweak: 'end-hung-apps-faster',
        title: 'End hung apps faster',
        turnedOn: true,
        status: 'rolledBack',
        detail: 'access denied',
        detailPhrase: null,
      },
    ],
    automation: 'notAttempted',
    complete: false,
  });
  render(<ProfilesScreen />);
  await user.click(await screen.findByRole('button', { name: 'Switch to Gaming' }));
  const dialog = await screen.findByRole('dialog', { name: 'Switch to Gaming' });
  await user.click(await within(dialog).findByRole('button', { name: 'Switch to Gaming' }));
  const alert = await within(dialog).findByRole('alert');
  expect(alert.textContent).toMatch(/stopped part-way: 1 of 2 tweaks were changed/);
  expect(within(dialog).getByText(/Failed, and put back as it was\. access denied/)).toBeTruthy();
  expect(within(dialog).getByText('The automation settings were not changed.')).toBeTruthy();
});

it('lets a tweak that needs administrator rights be seen but not chosen, and saves the rest', async () => {
  const user = userEvent.setup();
  ipc.saveProfile.mockResolvedValue(view({ suggested: false }));
  render(<ProfilesScreen />);
  await user.click(await screen.findByRole('button', { name: 'New profile' }));
  const dialog = await screen.findByRole('dialog', { name: 'New profile' });
  const admin = within(dialog).getByRole('combobox', {
    name: 'What the profile does with Turn off search indexing',
  });
  expect(admin.hasAttribute('disabled')).toBe(true);
  expect(within(dialog).getByText('Not included: needs administrator rights')).toBeTruthy();

  await user.type(within(dialog).getByRole('textbox', { name: 'Name' }), 'Mine');
  await user.selectOptions(
    within(dialog).getByRole('combobox', {
      name: 'What the profile does with Show file extensions',
    }),
    'on',
  );
  await user.type(within(dialog).getByPlaceholderText('game.exe'), 'editor.exe');
  await user.click(within(dialog).getByRole('button', { name: 'Save' }));

  expect(ipc.saveProfile).toHaveBeenCalledWith({
    id: '',
    name: 'Mine',
    tweaks: [{ id: 'show-file-extensions', enabled: true }],
    automation: { memory: null, cleaning: null },
    trigger: { exe: 'editor.exe', returnOnExit: true },
  });
  await vi.waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
});

it('offers to turn the active profile off', async () => {
  const user = userEvent.setup();
  ipc.listProfiles.mockResolvedValue(view({ active: 'gaming', suggested: false }));
  ipc.previewProfileSwitch.mockResolvedValue({ ...preview, to: null, items: [], automation: null });
  render(<ProfilesScreen />);
  expect(await screen.findByText('Active profile: Gaming')).toBeTruthy();
  await user.click(screen.getByRole('button', { name: 'Turn off profile' }));
  const dialog = await screen.findByRole('dialog', { name: 'Turn off Gaming' });
  expect(ipc.previewProfileSwitch).toHaveBeenCalledWith(null);
  expect(await within(dialog).findByText(/Nothing needs to change on this PC/)).toBeTruthy();
});
