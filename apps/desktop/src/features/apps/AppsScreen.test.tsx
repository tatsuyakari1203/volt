import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { AppListView, AppView, UninstallResult } from '../../shared/ipc';
import { IpcCommandError } from '../../shared/ipc';
import { ToastProvider } from '../../shared/ui';
import { AppsScreen } from './AppsScreen';

const ipc = vi.hoisted(() => ({
  listApps: vi.fn(),
  uninstallApp: vi.fn(),
}));
vi.mock('../../shared/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../shared/ipc')>()),
  ...ipc,
}));

const MB = 1024 ** 2;

function app(over: Partial<AppView> & Pick<AppView, 'id'>): AppView {
  return {
    name: over.id,
    publisher: 'Contoso',
    version: '1.0',
    sizeBytes: null,
    installed: null,
    source: 'machine64',
    hidden: null,
    uninstallable: true,
    ...over,
  };
}

function list(apps: AppView[], problems: AppListView['problems'] = []): AppListView {
  return { apps, problems };
}

const sample = list([
  app({
    id: 'win32:m64:a',
    name: 'Alpha Editor',
    publisher: 'Alpha Soft',
    version: '2.1',
    sizeBytes: 120 * MB,
    installed: '2024-02-29',
  }),
  app({
    id: 'win32:m64:b',
    name: 'Beta Player',
    publisher: 'Beta Media',
    sizeBytes: 900 * MB,
    installed: '2023-01-05',
  }),
  app({ id: 'win32:user:c', name: 'Gamma Notes', source: 'currentUser', installed: '2025-06-01' }),
  app({ id: 'pkg:d', name: 'Delta Photos', source: 'packaged', publisher: null }),
  app({
    id: 'win32:m32:e',
    name: 'Epsilon Runtime',
    hidden: 'systemComponent',
    sizeBytes: 5 * MB,
  }),
  app({
    id: 'win32:m64:f',
    name: 'Zeta Update',
    hidden: 'noUninstallCommand',
    uninstallable: false,
  }),
]);

function result(
  over: Partial<UninstallResult> & Pick<UninstallResult, 'id' | 'name'>,
): UninstallResult {
  return {
    source: 'machine64',
    outcome: 'removed',
    exitCode: 0,
    recorded: true,
    listing: sample,
    ...over,
  };
}

function renderScreen() {
  return render(
    <ToastProvider>
      <AppsScreen />
    </ToastProvider>,
  );
}

/** The app names in the order the rows are drawn. */
function order(): string[] {
  const rows = within(screen.getByRole('list', { name: 'Installed apps' })).getAllByRole(
    'listitem',
  );
  return rows.map((row) => row.querySelector('span.font-semibold')?.textContent ?? '');
}

beforeEach(() => {
  ipc.listApps.mockReset();
  ipc.uninstallApp.mockReset();
  ipc.listApps.mockResolvedValue(sample);
});

describe('the list', () => {
  it('shows apps by name with publisher, version, size and date, and hides system components', async () => {
    renderScreen();
    await screen.findByRole('list', { name: 'Installed apps' });
    expect(order()).toEqual(['Alpha Editor', 'Beta Player', 'Delta Photos', 'Gamma Notes']);
    const alpha = screen.getByRole('checkbox', { name: 'Select Alpha Editor' }).closest('li');
    expect(alpha?.textContent).toContain('Alpha Soft');
    expect(alpha?.textContent).toContain('Version 2.1');
    expect(alpha?.textContent).toContain('120.0 MB');
    expect(alpha?.textContent).toMatch(/Installed .*2024/);
    expect(screen.getByText('4 apps shown, 2 hidden as system components')).toBeTruthy();
    expect(screen.getByText('Publisher unknown')).toBeTruthy();
  });

  it('marks current-user and Store entries', async () => {
    renderScreen();
    await screen.findByRole('list', { name: 'Installed apps' });
    const gamma = screen.getByRole('checkbox', { name: 'Select Gamma Notes' }).closest('li');
    expect(within(gamma as HTMLElement).getByText('Current user')).toBeTruthy();
    const delta = screen.getByRole('checkbox', { name: 'Select Delta Photos' }).closest('li');
    expect(within(delta as HTMLElement).getByText('Store app')).toBeTruthy();
  });

  it('shows system components only behind the switch, and cannot select what Volt cannot remove', async () => {
    const user = userEvent.setup();
    renderScreen();
    await screen.findByRole('list', { name: 'Installed apps' });
    await user.click(screen.getByRole('switch', { name: 'Show system components' }));
    expect(order()).toContain('Epsilon Runtime');
    expect(order()).toContain('Zeta Update');
    const runtime = screen.getByRole('checkbox', { name: 'Select Epsilon Runtime' });
    expect((runtime as HTMLInputElement).disabled).toBe(false);
    const update = screen.getByRole('checkbox', { name: 'Select Zeta Update' });
    expect((update as HTMLInputElement).disabled).toBe(true);
    const row = update.closest('li') as HTMLElement;
    expect(within(row).getByText('No uninstaller')).toBeTruthy();
    expect(within(row).getByText('Volt cannot remove this one.')).toBeTruthy();
    expect(screen.getByText('6 apps shown')).toBeTruthy();
  });

  it('searches by name or publisher and says when nothing matches', async () => {
    const user = userEvent.setup();
    renderScreen();
    await screen.findByRole('list', { name: 'Installed apps' });
    await user.type(screen.getByRole('searchbox', { name: 'Search apps' }), 'beta media');
    await waitFor(() => expect(order()).toEqual(['Beta Player']));
    await user.clear(screen.getByRole('searchbox', { name: 'Search apps' }));
    await user.type(screen.getByRole('searchbox', { name: 'Search apps' }), 'nothing like this');
    expect(await screen.findByText('No app matches "nothing like this".')).toBeTruthy();
  });

  it('sorts by size and by date, biggest and newest first, unknown last', async () => {
    const user = userEvent.setup();
    renderScreen();
    await screen.findByRole('list', { name: 'Installed apps' });
    await user.click(screen.getByRole('radio', { name: 'Size' }));
    expect(order()).toEqual(['Beta Player', 'Alpha Editor', 'Delta Photos', 'Gamma Notes']);
    await user.click(screen.getByRole('radio', { name: 'Date' }));
    expect(order()).toEqual(['Gamma Notes', 'Alpha Editor', 'Beta Player', 'Delta Photos']);
    await user.click(screen.getByRole('radio', { name: 'Name' }));
    expect(order()[0]).toBe('Alpha Editor');
  });

  it('says which sources could not be read', async () => {
    ipc.listApps.mockResolvedValue(
      list(sample.apps, [{ source: 'packaged', unreadable: true, skipped: 0 }]),
    );
    renderScreen();
    expect(
      await screen.findByText('Some apps may be missing: the Store apps could not be read.'),
    ).toBeTruthy();
  });

  it('shows an error with a way to try again', async () => {
    const user = userEvent.setup();
    ipc.listApps.mockRejectedValueOnce(new Error('registry is down'));
    renderScreen();
    expect(await screen.findByText('registry is down')).toBeTruthy();
    await user.click(screen.getByRole('button', { name: 'Try again' }));
    expect(await screen.findByRole('list', { name: 'Installed apps' })).toBeTruthy();
  });
});

describe('uninstalling', () => {
  async function tick(user: ReturnType<typeof userEvent.setup>, ...names: string[]) {
    await screen.findByRole('list', { name: 'Installed apps' });
    for (const name of names) {
      await user.click(screen.getByRole('checkbox', { name: `Select ${name}` }));
    }
  }

  it('asks first, naming every app and saying Volt cannot undo it, and starts nothing on Cancel', async () => {
    const user = userEvent.setup();
    renderScreen();
    await tick(user, 'Alpha Editor', 'Beta Player');
    expect(screen.getByText('2 apps selected')).toBeTruthy();
    await user.click(screen.getByRole('button', { name: 'Uninstall 2 apps' }));
    const dialog = screen.getByRole('dialog', { name: 'Uninstall these apps?' });
    expect(within(dialog).getByText(/cannot be undone by Volt/)).toBeTruthy();
    const names = within(within(dialog).getByRole('list', { name: 'Apps to uninstall' }))
      .getAllByRole('listitem')
      .map((item) => item.textContent);
    expect(names).toEqual(['Alpha Editor', 'Beta Player']);
    await user.click(within(dialog).getByRole('button', { name: 'Cancel' }));
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(ipc.uninstallApp).not.toHaveBeenCalled();
  });

  it('runs the queue one app at a time, in order, and reports how each ended', async () => {
    const user = userEvent.setup();
    const gates: Array<(value: UninstallResult) => void> = [];
    ipc.uninstallApp.mockImplementation(
      () => new Promise<UninstallResult>((resolve) => gates.push(resolve)),
    );
    renderScreen();
    await tick(user, 'Alpha Editor', 'Beta Player', 'Gamma Notes');
    await user.click(screen.getByRole('button', { name: 'Uninstall 3 apps' }));
    await user.click(
      within(screen.getByRole('dialog')).getByRole('button', { name: 'Uninstall 3 apps' }),
    );

    // Only the first one has started.
    await waitFor(() => expect(ipc.uninstallApp).toHaveBeenCalledTimes(1));
    expect(ipc.uninstallApp).toHaveBeenLastCalledWith('win32:m64:a');
    const queue = screen.getByRole('region', { name: 'Uninstalling' });
    expect(within(queue).getAllByText('Waiting for its uninstaller to finish...')).toHaveLength(1);
    // The other two wait their turn.
    expect(within(queue).getAllByText('Waiting', { selector: 'p' })).toHaveLength(2);

    // The list that comes back with a result replaces the one on screen.
    const afterFirst = list(sample.apps.filter((a) => a.id !== 'win32:m64:a'));
    gates[0]?.(result({ id: 'win32:m64:a', name: 'Alpha Editor', listing: afterFirst }));
    await waitFor(() => expect(ipc.uninstallApp).toHaveBeenCalledTimes(2));
    expect(ipc.uninstallApp).toHaveBeenLastCalledWith('win32:m64:b');
    expect(screen.queryByRole('checkbox', { name: 'Select Alpha Editor' })).toBeNull();

    gates[1]?.(
      result({ id: 'win32:m64:b', name: 'Beta Player', outcome: 'stillInstalled', exitCode: 1602 }),
    );
    await waitFor(() => expect(ipc.uninstallApp).toHaveBeenCalledTimes(3));
    gates[2]?.(
      result({ id: 'win32:user:c', name: 'Gamma Notes', source: 'currentUser', outcome: 'failed' }),
    );

    expect(await within(queue).findByText('Finished')).toBeTruthy();
    expect(within(queue).getByText('Removed')).toBeTruthy();
    expect(within(queue).getByText(/Still installed\./)).toBeTruthy();
    expect(within(queue).getByText('Could not start the uninstaller.')).toBeTruthy();
    expect(
      within(queue).getByText('Removed 1, still installed 1, not removed 1, skipped 0.'),
    ).toBeTruthy();
    await user.click(within(queue).getByRole('button', { name: 'Done' }));
    expect(screen.queryByRole('region', { name: 'Uninstalling' })).toBeNull();
  });

  it('stops after the uninstaller that is running and skips the rest', async () => {
    const user = userEvent.setup();
    let finish: (value: UninstallResult) => void = () => {};
    ipc.uninstallApp.mockImplementation(
      () =>
        new Promise<UninstallResult>((resolve) => {
          finish = resolve;
        }),
    );
    renderScreen();
    await tick(user, 'Alpha Editor', 'Beta Player');
    await user.click(screen.getByRole('button', { name: 'Uninstall 2 apps' }));
    await user.click(
      within(screen.getByRole('dialog')).getByRole('button', { name: 'Uninstall 2 apps' }),
    );
    await waitFor(() => expect(ipc.uninstallApp).toHaveBeenCalledTimes(1));

    await user.click(screen.getByRole('button', { name: 'Stop after this one' }));
    expect(screen.getByRole('button', { name: 'Stopping after this one...' })).toBeTruthy();
    // The running uninstaller is not interrupted.
    finish(result({ id: 'win32:m64:a', name: 'Alpha Editor' }));

    const queue = screen.getByRole('region', { name: 'Uninstalling' });
    expect(await within(queue).findByText('Skipped.')).toBeTruthy();
    expect(ipc.uninstallApp).toHaveBeenCalledTimes(1);
    expect(
      within(queue).getByText('Removed 1, still installed 0, not removed 0, skipped 1.'),
    ).toBeTruthy();
  });

  it('reports an app that is already gone and a refused command without stopping the queue', async () => {
    const user = userEvent.setup();
    ipc.uninstallApp
      .mockRejectedValueOnce(
        new IpcCommandError({ kind: 'notFound', message: 'that app is not installed any more' }),
      )
      .mockRejectedValueOnce(
        new IpcCommandError({
          kind: 'invalidInput',
          message: 'this app cannot be removed from Volt',
        }),
      );
    renderScreen();
    await tick(user, 'Alpha Editor', 'Beta Player');
    await user.click(screen.getByRole('button', { name: 'Uninstall 2 apps' }));
    await user.click(
      within(screen.getByRole('dialog')).getByRole('button', { name: 'Uninstall 2 apps' }),
    );
    const queue = screen.getByRole('region', { name: 'Uninstalling' });
    expect(await within(queue).findByText('Already gone.')).toBeTruthy();
    expect(
      await within(queue).findByText(
        'Could not uninstall: Invalid input: this app cannot be removed from Volt',
      ),
    ).toBeTruthy();
    expect(ipc.uninstallApp).toHaveBeenCalledTimes(2);
  });
});
