import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';
import type { ProfilesView } from '../../shared/ipc';
import { ProfileSwitchCard } from './ProfileSwitchCard';

const ipc = vi.hoisted(() => ({
  listProfiles: vi.fn(),
  undoProfileSwitch: vi.fn(),
}));
vi.mock('../../shared/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../shared/ipc')>()),
  ...ipc,
}));

function view(over: Partial<ProfilesView> = {}): ProfilesView {
  const profile = (id: string, name: string) => ({
    id,
    name,
    tweaks: [],
    automation: { memory: null, cleaning: null },
    trigger: null,
  });
  return {
    profiles: [profile('gaming', 'Gaming'), profile('work', 'Work')],
    suggested: false,
    active: null,
    lastSwitch: null,
    lastSkip: null,
    watching: false,
    pollSeconds: 15,
    maxProfiles: 24,
    maxNameChars: 40,
    ...over,
  };
}

beforeEach(() => {
  for (const mock of Object.values(ipc)) mock.mockReset();
  ipc.listProfiles.mockResolvedValue(view());
});

it('lists the last switch on the Undo page and undoes all of it after a confirmation', async () => {
  const user = userEvent.setup();
  const onUndone = vi.fn();
  ipc.listProfiles.mockResolvedValue(
    view({
      active: 'gaming',
      lastSwitch: {
        at: 1_800_000_000_000,
        source: 'manual',
        from: 'work',
        to: 'gaming',
        applied: [{ id: 'faster-menu-display', title: 'Faster menu display' }],
        reverted: [{ id: 'show-file-extensions', title: 'Show file extensions' }],
        automationChanged: true,
        complete: true,
      },
    }),
  );
  ipc.undoProfileSwitch.mockResolvedValue({
    from: 'gaming',
    to: 'work',
    steps: [],
    automation: 'set',
    complete: true,
  });
  render(<ProfileSwitchCard onUndone={onUndone} />);
  const card = await screen.findByRole('region', { name: 'Last profile switch' });
  expect(
    within(card).getByText(/Turned on: Faster menu display\. Turned off: Show file extensions\./),
  ).toBeTruthy();
  await user.click(within(card).getByRole('button', { name: 'Undo this switch' }));
  const dialog = await screen.findByRole('dialog', { name: 'Undo the profile switch' });
  expect(within(dialog).getByText('Will be turned off: Faster menu display.')).toBeTruthy();
  expect(within(dialog).getByText('Will be turned on again: Show file extensions.')).toBeTruthy();
  expect(within(dialog).getByText('The automation settings go back too.')).toBeTruthy();
  expect(ipc.undoProfileSwitch).not.toHaveBeenCalled();
  await user.click(within(dialog).getByRole('button', { name: 'Undo switch' }));
  expect(ipc.undoProfileSwitch).toHaveBeenCalledTimes(1);
  await vi.waitFor(() => expect(onUndone).toHaveBeenCalled());
});

it('shows nothing on the Undo page when no profile was switched', async () => {
  render(<ProfileSwitchCard onUndone={() => {}} />);
  await vi.waitFor(() => expect(ipc.listProfiles).toHaveBeenCalled());
  expect(screen.queryByRole('region', { name: 'Last profile switch' })).toBeNull();
});
