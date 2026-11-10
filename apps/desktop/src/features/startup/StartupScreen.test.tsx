import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type {
  StartupChangeResult,
  StartupEntryView,
  StartupList,
  StartupProblemView,
} from '../../shared/ipc';
import { IpcCommandError, IpcUnavailableError } from '../../shared/ipc';
import { ToastProvider } from '../../shared/ui';
import { describeChange, shortenCommand } from './messages';
import { StartupScreen } from './StartupScreen';

const ipc = vi.hoisted(() => ({
  listStartupEntries: vi.fn(),
  setStartupEntryEnabled: vi.fn(),
}));
vi.mock('../../shared/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../shared/ipc')>()),
  ...ipc,
}));

const sync: StartupEntryView = {
  id: 'user-run/Sample Sync',
  name: 'Sample Sync',
  source: 'userRun',
  sourceLabel: 'Run (you)',
  command: '"C:\\Program Files\\Sample Sync\\sync.exe" --background',
  publisher: 'Sample Software Ltd',
  enabled: true,
  needsAdmin: false,
};

const updater: StartupEntryView = {
  id: 'machine-run/Vendor Updater',
  name: 'Vendor Updater',
  source: 'machineRun',
  sourceLabel: 'Run (all users)',
  command: '%ProgramFiles%\\Vendor\\update.exe /check',
  publisher: null,
  enabled: true,
  needsAdmin: true,
};

const disabledNotes: StartupEntryView = {
  id: 'user-folder/Notes.lnk',
  name: 'Notes',
  source: 'userFolder',
  sourceLabel: 'Startup folder (you)',
  command:
    'C:\\Users\\Sample\\AppData\\Roaming\\Microsoft\\Windows\\Start Menu\\Programs\\Startup\\Notes.lnk',
  publisher: null,
  enabled: false,
  needsAdmin: false,
};

function list(entries: StartupEntryView[], problems: StartupProblemView[] = []): StartupList {
  return { entries, problems };
}

function changed(
  entry: StartupEntryView,
  enabled: boolean | null,
  status: StartupChangeResult['status'] = 'changed',
): StartupChangeResult {
  return { id: entry.id, name: entry.name, status, enabled };
}

/** A promise the test settles by hand, to observe the state while a command runs. */
function deferred<T>() {
  let resolve: (value: T) => void = () => {};
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

function renderScreen() {
  return render(
    <ToastProvider>
      <StartupScreen />
    </ToastProvider>,
  );
}

beforeEach(() => {
  ipc.listStartupEntries.mockReset();
  ipc.setStartupEntryEnabled.mockReset();
});

describe('loading, empty and error states', () => {
  it('shows a loading state, then the list read from the shell', async () => {
    const pending = deferred<StartupList>();
    ipc.listStartupEntries.mockReturnValue(pending.promise);
    renderScreen();
    expect(screen.getByRole('status').textContent).toContain('Reading what starts with Windows');
    expect(screen.queryByRole('list', { name: 'Startup entries' })).toBeNull();

    pending.resolve(list([sync, disabledNotes]));
    const rows = await screen.findByRole('list', { name: 'Startup entries' });
    expect(within(rows).getAllByRole('listitem')).toHaveLength(2);
    expect(screen.queryByRole('status')).toBeNull();
    expect(screen.getByText('2 entries, 1 on')).toBeTruthy();
  });

  it('says so when nothing starts with Windows', async () => {
    ipc.listStartupEntries.mockResolvedValue(list([]));
    renderScreen();
    expect(await screen.findByText('Nothing starts with Windows')).toBeTruthy();
    expect(screen.queryByRole('searchbox')).toBeNull();
  });

  it('explains a failed read and offers Try again, which reads again', async () => {
    const user = userEvent.setup();
    ipc.listStartupEntries.mockRejectedValueOnce(
      new IpcCommandError({ kind: 'other', message: 'the registry is busy' }),
    );
    renderScreen();
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('Startup entries could not be loaded');
    expect(alert.textContent).toContain('the registry is busy');

    ipc.listStartupEntries.mockResolvedValue(list([sync]));
    await user.click(within(alert).getByRole('button', { name: 'Try again' }));
    expect(await screen.findByRole('switch', { name: 'Sample Sync' })).toBeTruthy();
    expect(ipc.listStartupEntries).toHaveBeenCalledTimes(2);
  });

  it('says it is only available inside the app when there is no bridge', async () => {
    ipc.listStartupEntries.mockRejectedValue(new IpcUnavailableError());
    renderScreen();
    expect((await screen.findByRole('alert')).textContent).toContain(
      'only available inside the Volt app',
    );
  });

  it('keeps the entries that could be read and names the places that could not', async () => {
    ipc.listStartupEntries.mockResolvedValue(
      list(
        [sync],
        [
          {
            source: 'machineRun32',
            sourceLabel: 'Run, 32-bit (all users)',
            reason: 'access denied',
          },
        ],
      ),
    );
    renderScreen();
    expect(await screen.findByRole('switch', { name: 'Sample Sync' })).toBeTruthy();
    expect(screen.getByRole('status').textContent).toContain(
      'Run, 32-bit (all users) (access denied)',
    );
  });
});

describe('what a row shows', () => {
  it('shows name, source badge, publisher, command and the state of the switch', async () => {
    ipc.listStartupEntries.mockResolvedValue(list([sync, disabledNotes]));
    renderScreen();
    const rows = await screen.findAllByRole('listitem');
    const first = within(rows[0] as HTMLElement);
    expect(first.getByText('Sample Sync')).toBeTruthy();
    expect(first.getByText('Run (you)')).toBeTruthy();
    expect(first.getByText('Publisher: Sample Software Ltd')).toBeTruthy();
    expect(first.getByText(sync.command)).toBeTruthy();
    expect(first.getByRole('switch', { name: 'Sample Sync' }).getAttribute('aria-checked')).toBe(
      'true',
    );
    const second = within(rows[1] as HTMLElement);
    expect(second.getByText('Publisher unknown')).toBeTruthy();
    expect(second.getByRole('switch', { name: 'Notes' }).getAttribute('aria-checked')).toBe(
      'false',
    );
  });

  it('shortens a long command and shows the full text on request', async () => {
    const user = userEvent.setup();
    ipc.listStartupEntries.mockResolvedValue(list([disabledNotes]));
    renderScreen();
    const toggle = await screen.findByRole('button', {
      name: 'Command of Notes: show the full command',
    });
    expect(toggle.textContent).not.toBe(disabledNotes.command);
    expect(toggle.textContent).toContain('…');
    expect(toggle.getAttribute('title')).toBe(disabledNotes.command);

    await user.click(toggle);
    const expanded = screen.getByRole('button', { name: 'Command of Notes: show less' });
    expect(expanded.textContent).toBe(disabledNotes.command);
    expect(expanded.getAttribute('aria-expanded')).toBe('true');
  });

  it('marks machine-wide rows as needing administrator rights and does not let them be changed', async () => {
    ipc.listStartupEntries.mockResolvedValue(list([updater]));
    renderScreen();
    const row = within(await screen.findByRole('listitem'));
    expect(row.getByText('Needs administrator rights')).toBeTruthy();
    expect(row.getByText('Run (all users)')).toBeTruthy();
    expect(row.getByRole('switch', { name: 'Vendor Updater' }).hasAttribute('disabled')).toBe(true);
  });

  it('filters by name, publisher, command and source', async () => {
    const user = userEvent.setup();
    ipc.listStartupEntries.mockResolvedValue(list([sync, updater, disabledNotes]));
    renderScreen();
    const search = await screen.findByRole('searchbox', { name: 'Search startup entries' });
    await user.type(search, 'sample software');
    expect(screen.getAllByRole('listitem')).toHaveLength(1);
    await user.clear(search);
    await user.type(search, 'nothing like this');
    expect(screen.getByText('No startup entries match "nothing like this"')).toBeTruthy();
  });
});

describe('changing an entry', () => {
  it('turns an entry off, shows the state the shell reports and says how to undo', async () => {
    const user = userEvent.setup();
    ipc.listStartupEntries.mockResolvedValue(list([sync]));
    const pending = deferred<StartupChangeResult>();
    ipc.setStartupEntryEnabled.mockReturnValue(pending.promise);
    renderScreen();
    const toggle = await screen.findByRole('switch', { name: 'Sample Sync' });

    await user.click(toggle);
    expect(ipc.setStartupEntryEnabled).toHaveBeenCalledWith('user-run/Sample Sync', false);
    // While the command runs the switch is locked and the row says what is happening.
    expect(toggle.hasAttribute('disabled')).toBe(true);
    expect(screen.getByText('Turning off...')).toBeTruthy();
    expect(toggle.getAttribute('aria-checked')).toBe('true');

    pending.resolve(changed(sync, false));
    await waitFor(() => expect(toggle.getAttribute('aria-checked')).toBe('false'));
    expect(toggle.hasAttribute('disabled')).toBe(false);
    expect(screen.queryByText('Turning off...')).toBeNull();
    expect(await screen.findByText(/will no longer start with Windows/)).toBeTruthy();
    expect(screen.getByText(/undo this on the Undo page/)).toBeTruthy();
    expect(screen.getByText('1 entry, 0 on')).toBeTruthy();
  });

  it('turns an entry back on', async () => {
    const user = userEvent.setup();
    ipc.listStartupEntries.mockResolvedValue(list([disabledNotes]));
    ipc.setStartupEntryEnabled.mockResolvedValue(changed(disabledNotes, true));
    renderScreen();
    const toggle = await screen.findByRole('switch', { name: 'Notes' });
    await user.click(toggle);
    expect(ipc.setStartupEntryEnabled).toHaveBeenCalledWith('user-folder/Notes.lnk', true);
    await waitFor(() => expect(toggle.getAttribute('aria-checked')).toBe('true'));
    expect(await screen.findByText(/will start with Windows again/)).toBeTruthy();
  });

  it('says plainly when nothing needed to change', async () => {
    const user = userEvent.setup();
    ipc.listStartupEntries.mockResolvedValue(list([sync]));
    ipc.setStartupEntryEnabled.mockResolvedValue(changed(sync, false, 'alreadyInState'));
    renderScreen();
    await user.click(await screen.findByRole('switch', { name: 'Sample Sync' }));
    expect(
      await screen.findByText('"Sample Sync" was already off. Nothing was changed.'),
    ).toBeTruthy();
  });

  it('reads the list again when the state could not be read back, and says so', async () => {
    const user = userEvent.setup();
    ipc.listStartupEntries
      .mockResolvedValueOnce(list([sync]))
      .mockResolvedValue(list([{ ...sync, enabled: false }]));
    ipc.setStartupEntryEnabled.mockResolvedValue(changed(sync, null));
    renderScreen();
    await user.click(await screen.findByRole('switch', { name: 'Sample Sync' }));
    expect((await screen.findByRole('alert')).textContent).toContain('could not read it back');
    await waitFor(() => expect(ipc.listStartupEntries).toHaveBeenCalledTimes(2));
  });

  it('explains a refusal for rights, reads the list again and leaves the switch as it was', async () => {
    const user = userEvent.setup();
    ipc.listStartupEntries.mockResolvedValue(list([sync]));
    ipc.setStartupEntryEnabled.mockRejectedValue(
      new IpcCommandError({ kind: 'accessDenied', message: 'registry key' }),
    );
    renderScreen();
    const toggle = await screen.findByRole('switch', { name: 'Sample Sync' });
    await user.click(toggle);
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('Windows refused to turn off "Sample Sync"');
    expect(toggle.getAttribute('aria-checked')).toBe('true');
    await waitFor(() => expect(ipc.listStartupEntries).toHaveBeenCalledTimes(2));
  });

  it('explains an entry that disappeared in the meantime', async () => {
    const user = userEvent.setup();
    ipc.listStartupEntries.mockResolvedValue(list([sync]));
    ipc.setStartupEntryEnabled.mockRejectedValue(
      new IpcCommandError({ kind: 'notFound', message: 'gone' }),
    );
    renderScreen();
    await user.click(await screen.findByRole('switch', { name: 'Sample Sync' }));
    expect((await screen.findByRole('alert')).textContent).toContain(
      '"Sample Sync" is no longer in Windows',
    );
  });

  it('explains any other failure with the reason from the shell', async () => {
    const user = userEvent.setup();
    ipc.listStartupEntries.mockResolvedValue(list([sync]));
    ipc.setStartupEntryEnabled.mockRejectedValue(
      new IpcCommandError({ kind: 'other', message: 'the undo journal is locked' }),
    );
    renderScreen();
    await user.click(await screen.findByRole('switch', { name: 'Sample Sync' }));
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('Could not turn off "Sample Sync"');
    expect(alert.textContent).toContain('the undo journal is locked');
    // The notice can be dismissed.
    await user.click(within(alert).getByRole('button', { name: 'Dismiss' }));
    expect(screen.queryByText(/Could not turn off/)).toBeNull();
  });
});

describe('messages', () => {
  it('words a refusal for administrator rights for a machine-wide row', async () => {
    const { describeChangeFailure } = await import('./messages');
    const text = describeChangeFailure(
      updater,
      false,
      new IpcCommandError({ kind: 'accessDenied', message: 'x' }),
    );
    expect(text).toBe(
      'Changing "Vendor Updater" needs administrator rights. Volt cannot do that yet.',
    );
  });

  it('words each change outcome', () => {
    expect(describeChange(sync, false, changed(sync, false)).message).toContain(
      'will no longer start with Windows',
    );
    expect(describeChange(sync, true, changed(sync, true)).tone).toBe('done');
    expect(describeChange(sync, true, changed(sync, null)).tone).toBe('problem');
    expect(describeChange(sync, true, changed(sync, true, 'alreadyInState')).message).toBe(
      '"Sample Sync" was already on. Nothing was changed.',
    );
  });

  it('shortens long commands in the middle and leaves short ones alone', () => {
    expect(shortenCommand('  notepad.exe  ')).toBe('notepad.exe');
    const long = `"C:\\Program Files\\Some Vendor\\Some Product\\bin\\launcher.exe" --silent --tray`;
    const short = shortenCommand(long, 40);
    expect(short.length).toBeLessThanOrEqual(40);
    expect(short.startsWith('"C:\\Program Files')).toBe(true);
    expect(short.endsWith('--tray')).toBe(true);
    expect(short).toContain('…');
  });
});
