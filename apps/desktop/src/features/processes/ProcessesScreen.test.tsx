import { act, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { EndAppResult, ProcessGroupView, ProcessListView } from '../../shared/ipc';
import { IpcUnavailableError } from '../../shared/ipc';
import { ToastProvider } from '../../shared/ui';
import { ProcessesScreen } from './ProcessesScreen';

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

function group(over: Partial<ProcessGroupView> & Pick<ProcessGroupView, 'id'>): ProcessGroupView {
  return {
    name: over.id,
    path: null,
    processCount: 1,
    endCount: 1,
    memoryBytes: 10 * MB,
    cpuPercent: 0,
    endability: 'allowed',
    ...over,
  };
}

function list(groups: ProcessGroupView[]): ProcessListView {
  return {
    groups,
    processCount: groups.reduce((sum, g) => sum + g.processCount, 0),
    logicalCpus: 8,
  };
}

const sample = list([
  group({ id: 'code.exe', memoryBytes: 900 * MB, cpuPercent: 1.5, processCount: 9, endCount: 9 }),
  group({
    id: 'chrome.exe',
    memoryBytes: 500 * MB,
    cpuPercent: 30,
    processCount: 12,
    endCount: 12,
  }),
  group({ id: 'explorer.exe', memoryBytes: 90 * MB, cpuPercent: 0.2, endability: 'explorer' }),
  group({
    id: 'svchost.exe',
    memoryBytes: 400 * MB,
    processCount: 60,
    endCount: 0,
    endability: 'protectedWindows',
  }),
]);

const ended: EndAppResult = {
  name: 'chrome.exe',
  ended: 12,
  needsAdministrator: 0,
  alreadyGone: 0,
  failed: 0,
  processes: [],
  recorded: true,
};

function renderScreen() {
  return render(
    <ToastProvider>
      <ProcessesScreen />
    </ToastProvider>,
  );
}

/** The app names in the order the rows are drawn. */
function order(): string[] {
  const rows = within(screen.getByRole('list', { name: 'Apps' })).getAllByRole('listitem');
  return rows.map((row) => row.querySelector('p')?.textContent ?? '');
}

beforeEach(() => {
  ipc.listProcesses.mockReset();
  ipc.endApp.mockReset();
  ipc.listProcesses.mockResolvedValue(sample);
  ipc.listProcessRules.mockReset();
  ipc.listProcessRules.mockResolvedValue([]);
});

afterEach(() => {
  vi.useRealTimers();
});

describe('ProcessesScreen', () => {
  it('lists apps by memory with process counts, memory and CPU, protected ones without End', async () => {
    renderScreen();
    await screen.findByRole('list', { name: 'Apps' });
    expect(order()).toEqual(['code.exe', 'chrome.exe', 'svchost.exe', 'explorer.exe']);
    expect(screen.getByText('4 apps, 82 processes')).toBeTruthy();

    const chrome = screen.getByRole('button', { name: 'End chrome.exe' }).closest('li');
    expect(chrome).toBeTruthy();
    const row = within(chrome as HTMLElement);
    expect(row.getByText('12 processes')).toBeTruthy();
    expect(row.getByText('500.0 MB')).toBeTruthy();
    expect(row.getByText('30%')).toBeTruthy();

    // A Windows-critical app is shown, labelled and has no End button.
    expect(screen.queryByRole('button', { name: 'End svchost.exe' })).toBeNull();
    const svchost = screen.getByText('svchost.exe').closest('li') as HTMLElement;
    expect(within(svchost).getByText('Protected')).toBeTruthy();
    expect(within(svchost).getByText('60 processes')).toBeTruthy();
  });

  it('sorts by CPU and searches by name', async () => {
    const user = userEvent.setup();
    renderScreen();
    await screen.findByRole('list', { name: 'Apps' });

    await user.click(screen.getByRole('radio', { name: 'CPU' }));
    expect(order()[0]).toBe('chrome.exe');

    await user.type(screen.getByRole('searchbox', { name: 'Search processes' }), 'R');
    // Only chrome.exe and explorer.exe contain an r, in any case.
    expect(order()).toEqual(['chrome.exe', 'explorer.exe']);
    expect(screen.getByText('2 of 4 apps match')).toBeTruthy();

    await user.type(screen.getByRole('searchbox', { name: 'Search processes' }), 'zzz');
    expect(await screen.findByText('No app matches "Rzzz".')).toBeTruthy();
  });

  it('asks before ending: names the app, the count and the unsaved work, and Cancel ends nothing', async () => {
    const user = userEvent.setup();
    renderScreen();
    await screen.findByRole('list', { name: 'Apps' });

    await user.click(screen.getByRole('button', { name: 'End chrome.exe' }));
    const dialog = screen.getByRole('dialog', { name: 'End chrome.exe?' });
    expect(dialog.textContent).toContain('12 processes of chrome.exe will end at once');
    expect(dialog.textContent).toContain('Anything you have not saved in it is lost');
    expect(dialog.textContent).toContain('This cannot be undone');
    expect(dialog.textContent).not.toContain('Windows Explorer');
    // The safe choice has focus.
    expect(document.activeElement).toBe(within(dialog).getByRole('button', { name: 'Cancel' }));

    await user.click(within(dialog).getByRole('button', { name: 'Cancel' }));
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(ipc.endApp).not.toHaveBeenCalled();
  });

  it('ends the app after the confirmation, says so and reads the list again', async () => {
    const user = userEvent.setup();
    // The screen also reads the list by itself every few seconds, on a real timer, so the number
    // of readings says nothing on a slow machine. Only the reading after the end can show the
    // new list, so the list changes when the app ends and the test waits for that.
    ipc.endApp.mockImplementation(() => {
      ipc.listProcesses.mockResolvedValue(list(sample.groups.filter((g) => g.id !== 'chrome.exe')));
      return Promise.resolve(ended);
    });
    renderScreen();
    await screen.findByRole('list', { name: 'Apps' });

    await user.click(screen.getByRole('button', { name: 'End chrome.exe' }));
    await user.click(screen.getByRole('button', { name: 'End 12 processes' }));

    expect(ipc.endApp).toHaveBeenCalledWith('chrome.exe');
    expect(await screen.findByText('Ended chrome.exe: 12 processes.')).toBeTruthy();
    expect(screen.queryByRole('dialog')).toBeNull();
    await waitFor(() => {
      expect(screen.queryByRole('button', { name: 'End chrome.exe' })).toBeNull();
    });
  });

  it('warns extra for Windows Explorer', async () => {
    const user = userEvent.setup();
    renderScreen();
    await screen.findByRole('list', { name: 'Apps' });
    await user.click(screen.getByRole('button', { name: 'End explorer.exe' }));
    const dialog = screen.getByRole('dialog', { name: 'End explorer.exe?' });
    expect(dialog.textContent).toContain('This is Windows Explorer');
    expect(dialog.textContent).toContain('taskbar');
  });

  it('reports processes that need administrator rights instead of claiming success', async () => {
    const user = userEvent.setup();
    ipc.endApp.mockResolvedValue({ ...ended, ended: 9, needsAdministrator: 3 });
    renderScreen();
    await screen.findByRole('list', { name: 'Apps' });
    await user.click(screen.getByRole('button', { name: 'End chrome.exe' }));
    await user.click(screen.getByRole('button', { name: 'End 12 processes' }));
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('3 processes need administrator rights');
  });

  it('shows the error when the core refuses', async () => {
    const user = userEvent.setup();
    ipc.endApp.mockRejectedValue(new Error('chrome.exe cannot be ended'));
    renderScreen();
    await screen.findByRole('list', { name: 'Apps' });
    await user.click(screen.getByRole('button', { name: 'End chrome.exe' }));
    await user.click(screen.getByRole('button', { name: 'End 12 processes' }));
    expect((await screen.findByRole('alert')).textContent).toContain('chrome.exe cannot be ended');
  });

  it('says so when processes cannot be read, and tries again on request', async () => {
    const user = userEvent.setup();
    ipc.listProcesses.mockRejectedValueOnce(new Error('boom'));
    renderScreen();
    expect((await screen.findByRole('alert')).textContent).toContain('boom');
    await user.click(screen.getByRole('button', { name: 'Try again' }));
    expect(await screen.findByRole('list', { name: 'Apps' })).toBeTruthy();
  });
});

describe('refreshing', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  async function advance(ms: number) {
    await act(async () => {
      await vi.advanceTimersByTimeAsync(ms);
    });
  }

  it('reads again every two seconds, keeps each row mounted and updates its numbers', async () => {
    renderScreen();
    await advance(0);
    const button = screen.getByRole('button', { name: 'End chrome.exe' });
    const row = button.closest('li');
    expect(screen.getByText('500.0 MB')).toBeTruthy();

    ipc.listProcesses.mockResolvedValue(
      list(sample.groups.map((g) => (g.id === 'chrome.exe' ? { ...g, memoryBytes: 650 * MB } : g))),
    );
    // The first follow-up reading comes after a second, the next ones every two.
    await advance(1000);
    expect(ipc.listProcesses).toHaveBeenCalledTimes(2);
    expect(screen.getByText('650.0 MB')).toBeTruthy();
    // The same DOM node: the row was updated, not mounted again.
    expect(screen.getByRole('button', { name: 'End chrome.exe' }).closest('li')).toBe(row);

    await advance(1999);
    expect(ipc.listProcesses).toHaveBeenCalledTimes(2);
    await advance(1);
    expect(ipc.listProcesses).toHaveBeenCalledTimes(3);
  });

  it('does not read while the window is hidden and reads at once when it is shown again', async () => {
    renderScreen();
    await advance(0);
    expect(ipc.listProcesses).toHaveBeenCalledTimes(1);

    const visibility = vi.spyOn(document, 'visibilityState', 'get');
    visibility.mockReturnValue('hidden');
    document.dispatchEvent(new Event('visibilitychange'));
    await advance(10_000);
    expect(ipc.listProcesses).toHaveBeenCalledTimes(1);

    visibility.mockReturnValue('visible');
    document.dispatchEvent(new Event('visibilitychange'));
    await advance(0);
    expect(ipc.listProcesses).toHaveBeenCalledTimes(2);
    visibility.mockRestore();
  });

  it('stops reading when the screen is left', async () => {
    const view = renderScreen();
    await advance(0);
    view.unmount();
    await advance(10_000);
    expect(ipc.listProcesses).toHaveBeenCalledTimes(1);
  });

  it('keeps the list and reports a failed update, and stops without the Volt bridge', async () => {
    renderScreen();
    await advance(0);
    ipc.listProcesses.mockRejectedValueOnce(new Error('busy'));
    await advance(1000);
    expect(screen.getByRole('list', { name: 'Apps' })).toBeTruthy();
    expect(screen.getByRole('alert').textContent).toContain('The list could not be updated: busy');
    ipc.listProcesses.mockResolvedValue(sample);
    await advance(2000);
    expect(screen.queryByRole('alert')).toBeNull();

    ipc.listProcesses.mockRejectedValue(new IpcUnavailableError());
    await advance(2000);
    const calls = ipc.listProcesses.mock.calls.length;
    await advance(10_000);
    expect(ipc.listProcesses.mock.calls.length).toBe(calls);
  });
});
