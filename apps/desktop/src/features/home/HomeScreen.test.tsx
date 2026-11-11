import { act, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { BoostResult, HealthView, MetricsView } from '../../shared/ipc';
import { IpcCommandError, IpcUnavailableError } from '../../shared/ipc';
import { HomeScreen } from './HomeScreen';

const GIB = 1024 ** 3;

const ipc = vi.hoisted(() => ({
  getHealthReport: vi.fn(),
  getLatestMetrics: vi.fn(),
  onMetricsUpdated: vi.fn(),
  runBoost: vi.fn(),
}));
vi.mock('../../shared/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../shared/ipc')>()),
  ...ipc,
}));

/** The handlers the screen registered for live readings, and the unlisten spy they returned. */
let listeners: Array<(metrics: MetricsView) => void> = [];
let unlisten = vi.fn();
let order: string[] = [];

/** A promise the test settles by hand, to observe the state while a command runs. */
function deferred<T>() {
  let resolve: (value: T) => void = () => {};
  let reject: (error: unknown) => void = () => {};
  const promise = new Promise<T>((done, fail) => {
    resolve = done;
    reject = fail;
  });
  return { promise, resolve, reject };
}

/** What the shell reports for a PC with a low disk, a busy memory and a restart waiting. */
function report(over: Partial<HealthView> = {}): HealthView {
  return {
    score: 58,
    level: 'poor',
    deductions: [
      {
        id: 'disk.low',
        points: 20,
        title: 'Disk space is low',
        reason: '24 GB of 476 GB is free on C: (5.0%). Windows needs room to install updates.',
        reasonPhrase: { code: 'health.disk', args: {} },
        fix: 'openCleaner',
      },
      {
        id: 'memory.high',
        points: 12,
        title: 'Memory use is high',
        reason: '85% of your memory is in use.',
        reasonPhrase: { code: 'health.memory', args: {} },
        fix: 'openMemory',
      },
      {
        id: 'restart.pending',
        points: 10,
        title: 'A restart is waiting',
        reason: 'Windows has updates or installs that finish only after a restart.',
        reasonPhrase: { code: 'health.restart', args: {} },
        fix: 'restartWindows',
      },
    ],
    unchecked: [],
    systemMount: 'C:\\',
    checkedAt: '2026-11-10T08:30:00.000Z',
    ...over,
  };
}

/** A reading of a machine with a GPU. */
function reading(over: Partial<MetricsView> = {}): MetricsView {
  return {
    takenAt: '2026-11-10T08:30:01.000Z',
    cpuPercent: 23.4,
    memory: {
      totalBytes: 16 * GIB,
      usedBytes: 9.9 * GIB,
      availableBytes: 6.1 * GIB,
      usedPercent: 61.9,
    },
    disks: [
      {
        mount: 'C:\\',
        totalBytes: 476 * GIB,
        usedBytes: 294 * GIB,
        freeBytes: 182 * GIB,
        usedPercent: 61.8,
      },
    ],
    gpu: {
      name: 'NVIDIA GeForce RTX 5060 Ti',
      utilisationPercent: 7.2,
      vramUsedBytes: 3.2 * GIB,
      vramTotalBytes: 16 * GIB,
      vramUsedPercent: 20,
    },
    ...over,
  };
}

function emit(metrics: MetricsView) {
  act(() => {
    for (const listener of listeners) listener(metrics);
  });
}

function card(name: string) {
  return screen.getByRole('article', { name });
}

function renderHome(onNavigate = vi.fn()) {
  render(<HomeScreen onNavigate={onNavigate} />);
  return onNavigate;
}

beforeEach(() => {
  listeners = [];
  order = [];
  unlisten = vi.fn();
  for (const mock of Object.values(ipc)) mock.mockReset();
  ipc.getHealthReport.mockResolvedValue(report());
  ipc.onMetricsUpdated.mockImplementation((handler: (metrics: MetricsView) => void) => {
    order.push('listen');
    listeners.push(handler);
    return Promise.resolve(unlisten);
  });
  ipc.getLatestMetrics.mockImplementation(() => {
    order.push('latest');
    return Promise.resolve(reading());
  });
});

describe('the health score', () => {
  it('shows the score in the ring with its grade and the headline', async () => {
    renderHome();
    const ring = await screen.findByRole('progressbar', { name: 'Health score' });
    expect(ring.getAttribute('aria-valuenow')).toBe('58');
    expect(within(ring).getByText('58')).toBeTruthy();
    expect(within(ring).getByText('Poor')).toBeTruthy();
    expect(screen.getByText('Your PC needs attention')).toBeTruthy();
    expect(screen.getByText(/3 things lower the score/)).toBeTruthy();
  });

  it('lists every deduction with its points and reason, and they add up to the score', async () => {
    renderHome();
    const list = await screen.findByRole('list', { name: 'Deductions' });
    const rows = within(list).getAllByRole('listitem');
    expect(rows).toHaveLength(3);

    expect(within(rows[0] as HTMLElement).getByText('Disk space is low')).toBeTruthy();
    expect(within(rows[0] as HTMLElement).getByText('−20 points')).toBeTruthy();
    expect(within(rows[0] as HTMLElement).getByText(/24 GB of 476 GB is free on C:/)).toBeTruthy();
    expect(within(rows[1] as HTMLElement).getByText('−12 points')).toBeTruthy();
    expect(within(rows[2] as HTMLElement).getByText('−10 points')).toBeTruthy();

    const shown = report();
    const points = shown.deductions.reduce((sum, deduction) => sum + deduction.points, 0);
    expect(100 - points).toBe(shown.score);
    expect(screen.getByText(/100 minus the points below/)).toBeTruthy();
  });

  it('sends the user to the section that fixes each deduction', async () => {
    const user = userEvent.setup();
    const onNavigate = renderHome();
    await user.click(await screen.findByRole('button', { name: /Open Cleaner/ }));
    expect(onNavigate).toHaveBeenLastCalledWith('clean');
    await user.click(screen.getByRole('button', { name: /Open Memory/ }));
    expect(onNavigate).toHaveBeenLastCalledWith('memory');
    expect(onNavigate).toHaveBeenCalledTimes(2);
  });

  it('sends a startup deduction to Startup', async () => {
    ipc.getHealthReport.mockResolvedValue(
      report({
        score: 85,
        level: 'good',
        deductions: [
          {
            id: 'startup.many',
            points: 15,
            title: 'Many apps start with Windows',
            reason: '5 apps that Windows rates as high impact start when you sign in.',
            reasonPhrase: { code: 'health.startup', args: {} },
            fix: 'openStartup',
          },
        ],
      }),
    );
    const user = userEvent.setup();
    const onNavigate = renderHome();
    await user.click(await screen.findByRole('button', { name: /Open Startup/ }));
    expect(onNavigate).toHaveBeenCalledWith('startup');
  });

  it('shows a restart as a sentence, because Volt has no restart button', async () => {
    renderHome();
    const row = (await screen.findByText('A restart is waiting')).closest('li') as HTMLElement;
    expect(within(row).getByText(/Restart Windows when you are ready/)).toBeTruthy();
    expect(within(row).queryByRole('button')).toBeNull();
  });

  it('lists a deduction whose fix this version does not know without a button', async () => {
    ipc.getHealthReport.mockResolvedValue(
      report({
        deductions: [
          {
            id: 'future.rule',
            points: 5,
            title: 'Something new',
            reason: 'A rule from a newer core.',
            reasonPhrase: { code: 'health.restart', args: {} },
            fix: null,
          },
        ],
      }),
    );
    renderHome();
    const row = (await screen.findByText('Something new')).closest('li') as HTMLElement;
    expect(within(row).queryByRole('button')).toBeNull();
  });

  it('says nothing is lowering a score of 100 and shows no deduction list', async () => {
    ipc.getHealthReport.mockResolvedValue(report({ score: 100, level: 'good', deductions: [] }));
    renderHome();
    const ring = await screen.findByRole('progressbar', { name: 'Health score' });
    expect(ring.getAttribute('aria-valuenow')).toBe('100');
    expect(screen.getByText('Your PC is in good shape')).toBeTruthy();
    expect(screen.getByText('Nothing is lowering the score.')).toBeTruthy();
    expect(screen.queryByRole('list', { name: 'Deductions' })).toBeNull();
  });

  it('says which checks could not run and that they are not part of the score', async () => {
    ipc.getHealthReport.mockResolvedValue(
      report({
        score: 100,
        level: 'good',
        deductions: [],
        unchecked: ['pendingRestart', 'systemDisk'],
      }),
    );
    renderHome();
    expect(
      await screen.findByText(
        'Volt could not check pending restarts, free disk space. They are not part of the score.',
      ),
    ).toBeTruthy();
  });

  it('gives no score when no check could run', async () => {
    ipc.getHealthReport.mockResolvedValue(
      report({
        score: 100,
        level: 'unknown',
        deductions: [],
        unchecked: ['systemDisk', 'memory', 'pendingRestart'],
      }),
    );
    renderHome();
    expect(await screen.findByText('Your PC could not be checked')).toBeTruthy();
    expect(screen.queryByRole('progressbar', { name: 'Health score' })).toBeNull();
    expect(screen.getByRole('img', { name: 'Health score not available' })).toBeTruthy();
    expect(screen.getByText('Not checked')).toBeTruthy();
  });

  it('shows a loading state until the first report arrives', async () => {
    const pending = deferred<HealthView>();
    ipc.getHealthReport.mockReturnValue(pending.promise);
    renderHome();
    expect(screen.getByText('Checking your PC...')).toBeTruthy();
    expect(screen.queryByRole('progressbar', { name: 'Health score' })).toBeNull();
    await act(async () => pending.resolve(report()));
    expect(await screen.findByRole('progressbar', { name: 'Health score' })).toBeTruthy();
  });

  it('explains a failed check and tries again on request', async () => {
    ipc.getHealthReport.mockRejectedValueOnce(
      new IpcCommandError({ kind: 'other', message: 'the background task failed' }),
    );
    const user = userEvent.setup();
    renderHome();
    const alert = await screen.findByRole('alert');
    expect(within(alert).getByText('Your PC could not be checked')).toBeTruthy();
    expect(within(alert).getByText('the background task failed')).toBeTruthy();

    await user.click(within(alert).getByRole('button', { name: 'Try again' }));
    expect(await screen.findByRole('progressbar', { name: 'Health score' })).toBeTruthy();
    expect(ipc.getHealthReport).toHaveBeenCalledTimes(2);
  });

  it('says it is only available inside the app when there is no Tauri bridge', async () => {
    ipc.getHealthReport.mockRejectedValue(new IpcUnavailableError());
    renderHome();
    expect(await screen.findByText('This is only available inside the Volt app.')).toBeTruthy();
  });

  it('checks again on request, keeps the old report on screen meanwhile and shows the new one', async () => {
    const user = userEvent.setup();
    renderHome();
    await screen.findByRole('progressbar', { name: 'Health score' });

    const next = deferred<HealthView>();
    ipc.getHealthReport.mockReturnValueOnce(next.promise);
    await user.click(screen.getByRole('button', { name: 'Check again' }));
    const busy = screen.getByRole('button', { name: 'Checking...' });
    expect(busy.getAttribute('aria-busy')).toBe('true');
    // The old score stays while the new check runs.
    expect(
      screen.getByRole('progressbar', { name: 'Health score' }).getAttribute('aria-valuenow'),
    ).toBe('58');

    await act(async () => next.resolve(report({ score: 100, level: 'good', deductions: [] })));
    await waitFor(() =>
      expect(
        screen.getByRole('progressbar', { name: 'Health score' }).getAttribute('aria-valuenow'),
      ).toBe('100'),
    );
    expect(screen.getByRole('button', { name: 'Check again' })).toBeTruthy();
  });

  it('ignores a second click while a check runs', async () => {
    const user = userEvent.setup();
    renderHome();
    await screen.findByRole('progressbar', { name: 'Health score' });
    const next = deferred<HealthView>();
    ipc.getHealthReport.mockReturnValueOnce(next.promise);
    const button = screen.getByRole('button', { name: 'Check again' });
    await user.click(button);
    await user.click(screen.getByRole('button', { name: 'Checking...' }));
    expect(ipc.getHealthReport).toHaveBeenCalledTimes(2);
    await act(async () => next.resolve(report()));
  });

  it('keeps the last report and says so when checking again fails', async () => {
    const user = userEvent.setup();
    renderHome();
    await screen.findByRole('progressbar', { name: 'Health score' });
    ipc.getHealthReport.mockRejectedValueOnce(new Error('registry unavailable'));
    await user.click(screen.getByRole('button', { name: 'Check again' }));
    expect(await screen.findByText('Could not check again. registry unavailable')).toBeTruthy();
    expect(screen.getByRole('progressbar', { name: 'Health score' })).toBeTruthy();
  });

  it('asks for the report once when the screen opens, not on a timer', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      renderHome();
      await screen.findByRole('progressbar', { name: 'Health score' });
      // Live readings arrive every second; none of them reads the score again.
      emit(reading());
      await act(async () => {
        await vi.advanceTimersByTimeAsync(10_000);
      });
      emit(reading({ cpuPercent: 80 }));
      expect(ipc.getHealthReport).toHaveBeenCalledTimes(1);
    } finally {
      vi.useRealTimers();
    }
  });
});

describe('the live cards', () => {
  it('registers the listener first and then asks for the latest reading', async () => {
    renderHome();
    await within(await screen.findByRole('region', { name: 'Live metrics' })).findByText('23%');
    expect(order).toEqual(['listen', 'latest']);
  });

  it('shows the four cards with their values', async () => {
    renderHome();
    await screen.findByText('23%');

    expect(within(card('CPU')).getByText('23%')).toBeTruthy();
    expect(within(card('Memory')).getByText('62%')).toBeTruthy();
    expect(within(card('Memory')).getByText('9.9 GB of 16 GB in use')).toBeTruthy();
    expect(within(card('GPU')).getByText('7%')).toBeTruthy();
    expect(within(card('GPU')).getByText('NVIDIA GeForce RTX 5060 Ti')).toBeTruthy();
    expect(within(card('GPU')).getByText('3.2 GB of 16 GB video memory')).toBeTruthy();
    expect(within(card('Disk space')).getByText('182 GB')).toBeTruthy();
    expect(within(card('Disk space')).getByText('Free of 476 GB on C:')).toBeTruthy();
  });

  it('follows the live readings', async () => {
    renderHome();
    await screen.findByText('23%');
    emit(reading({ cpuPercent: 91.2 }));
    expect(within(card('CPU')).getByText('91%')).toBeTruthy();
    expect(within(card('CPU')).queryByText('23%')).toBeNull();
  });

  it('shows a dash and an honest line for a CPU the shell could not work out yet', async () => {
    ipc.getLatestMetrics.mockResolvedValue(reading({ cpuPercent: null }));
    renderHome();
    await screen.findByText('62%');
    expect(within(card('CPU')).getByText('—')).toBeTruthy();
    expect(within(card('CPU')).getByText('No reading yet')).toBeTruthy();
    expect(within(card('CPU')).queryByText(/%/)).toBeNull();
  });

  it('says No GPU detected on a machine without one and invents no number', async () => {
    ipc.getLatestMetrics.mockResolvedValue(reading({ gpu: null }));
    renderHome();
    await screen.findByText('62%');
    const gpu = card('GPU');
    expect(within(gpu).getByText('—')).toBeTruthy();
    expect(within(gpu).getByText('No GPU detected')).toBeTruthy();
    expect(within(gpu).queryByText(/%|GB/)).toBeNull();
  });

  it('names a GPU whose usage the driver does not report, with a dash for the figure', async () => {
    ipc.getLatestMetrics.mockResolvedValue(
      reading({
        gpu: {
          name: 'Intel UHD Graphics',
          utilisationPercent: null,
          vramUsedBytes: null,
          vramTotalBytes: null,
          vramUsedPercent: null,
        },
      }),
    );
    renderHome();
    await screen.findByText('62%');
    const gpu = card('GPU');
    expect(within(gpu).getByText('—')).toBeTruthy();
    expect(within(gpu).getByText('Intel UHD Graphics')).toBeTruthy();
    expect(within(gpu).getByText('Usage is not available')).toBeTruthy();
    expect(within(gpu).queryByText(/video memory/)).toBeNull();
  });

  it('shows the system disk named by the report when there are several disks', async () => {
    ipc.getLatestMetrics.mockResolvedValue(
      reading({
        disks: [
          {
            mount: 'D:\\',
            totalBytes: 2000 * GIB,
            usedBytes: 1000 * GIB,
            freeBytes: 1000 * GIB,
            usedPercent: 50,
          },
          {
            mount: 'C:\\',
            totalBytes: 476 * GIB,
            usedBytes: 294 * GIB,
            freeBytes: 182 * GIB,
            usedPercent: 61.8,
          },
        ],
      }),
    );
    renderHome();
    await screen.findByText('62%');
    await waitFor(() =>
      expect(within(card('Disk space')).getByText('Free of 476 GB on C:')).toBeTruthy(),
    );
  });

  it('shows a loading state in each card until the first reading', async () => {
    ipc.getLatestMetrics.mockResolvedValue(null);
    renderHome();
    await screen.findByRole('progressbar', { name: 'Health score' });
    for (const name of ['CPU', 'Memory', 'GPU', 'Disk space']) {
      expect(card(name).getAttribute('aria-busy')).toBe('true');
      expect(within(card(name)).getByText('Waiting for the first reading')).toBeTruthy();
    }
    emit(reading());
    expect(within(card('CPU')).getByText('23%')).toBeTruthy();
    expect(card('CPU').getAttribute('aria-busy')).toBeNull();
  });

  it('does not let the older latest reading overwrite a newer one from an event', async () => {
    const latest = deferred<MetricsView | null>();
    ipc.getLatestMetrics.mockReturnValue(latest.promise);
    renderHome();
    await waitFor(() => expect(listeners).toHaveLength(1));
    await waitFor(() => expect(ipc.getLatestMetrics).toHaveBeenCalled());
    emit(reading({ cpuPercent: 77 }));
    await act(async () => latest.resolve(reading({ cpuPercent: 5 })));
    expect(within(card('CPU')).getByText('77%')).toBeTruthy();
  });

  it('explains when the shell cannot be reached and offers to try again', async () => {
    ipc.onMetricsUpdated.mockRejectedValueOnce(new IpcUnavailableError());
    const user = userEvent.setup();
    renderHome();
    const section = await screen.findByRole('region', { name: 'Live metrics' });
    const alert = await within(section).findByRole('alert');
    expect(within(alert).getByText('Live numbers are not available')).toBeTruthy();
    expect(within(alert).getByText('This is only available inside the Volt app.')).toBeTruthy();
    // The health score is unaffected.
    expect(await screen.findByRole('progressbar', { name: 'Health score' })).toBeTruthy();

    await user.click(within(alert).getByRole('button', { name: 'Try again' }));
    expect(await within(section).findByText('23%')).toBeTruthy();
  });

  it('explains a failure to read the latest reading when no event has arrived', async () => {
    ipc.getLatestMetrics.mockRejectedValue(
      new IpcCommandError({ kind: 'other', message: 'metrics are unavailable' }),
    );
    renderHome();
    const alert = await screen.findByText('metrics are unavailable');
    expect(alert).toBeTruthy();
    // A reading that arrives afterwards replaces the failure.
    emit(reading());
    expect(await screen.findByText('23%')).toBeTruthy();
    expect(screen.queryByText('Live numbers are not available')).toBeNull();
  });

  it('stops listening when the screen goes away', async () => {
    const { unmount } = render(<HomeScreen onNavigate={vi.fn()} />);
    await screen.findByText('23%');
    unmount();
    expect(unlisten).toHaveBeenCalledTimes(1);
  });

  it('stops listening even when the screen goes away before the listener is registered', async () => {
    const registered = deferred<() => void>();
    ipc.onMetricsUpdated.mockReturnValue(registered.promise);
    const { unmount } = render(<HomeScreen onNavigate={vi.fn()} />);
    unmount();
    await act(async () => registered.resolve(unlisten));
    expect(unlisten).toHaveBeenCalledTimes(1);
    expect(ipc.getLatestMetrics).not.toHaveBeenCalled();
  });
});

describe('the screen as a whole', () => {
  it('has no sample marker: it shows real data only', async () => {
    renderHome();
    await screen.findByRole('progressbar', { name: 'Health score' });
    expect(screen.queryByText(/sample data/i)).toBeNull();
    expect(screen.getByRole('heading', { level: 1, name: 'Home' })).toBeTruthy();
  });
});

function boostResult(over: Partial<BoostResult> = {}): BoostResult {
  return {
    memoryBefore: null,
    memoryAfter: null,
    availableBefore: 4 * GIB,
    availableAfter: 5.5 * GIB,
    areas: [
      {
        area: 'workingSets',
        status: 'done',
        processesTrimmed: 41,
        processesSkipped: 3,
        processesExcluded: 0,
        detail: null,
      },
    ],
    skipped: [
      {
        area: 'standbyList',
        state: 'needsAdministrator',
        reason: 'Needs administrator rights. Volt is not running with them.',
      },
    ],
    junk: { files: 1200, bytes: 340 * 1024 ** 2, truncated: false },
    movedToBin: null,
    recorded: true,
    ...over,
  };
}

describe('Boost', () => {
  it('runs with one click and reports memory before and after, the skipped operation and the junk found', async () => {
    const user = userEvent.setup();
    ipc.runBoost.mockResolvedValue(boostResult());
    const onNavigate = renderHome();
    await user.click(screen.getByRole('button', { name: 'Boost' }));

    const boost = await screen.findByRole('region', { name: 'Boost report' });
    expect(ipc.runBoost).toHaveBeenCalledTimes(1);
    expect(within(boost).getByText('4 GB')).toBeTruthy();
    expect(within(boost).getByText('5.5 GB')).toBeTruthy();
    expect(within(boost).getByText(/41 programs trimmed/)).toBeTruthy();
    expect(within(boost).getByText(/Skipped\. Needs administrator rights/)).toBeTruthy();
    expect(within(boost).getByText(/1,200 files \(340\.0 MB\) could be moved/)).toBeTruthy();
    expect(within(boost).getByText(/Nothing was cleaned/)).toBeTruthy();
    expect(within(boost).queryByText(/freed/i)).toBeNull();
    await user.click(within(boost).getByRole('button', { name: 'Open Cleaner' }));
    expect(onNavigate).toHaveBeenCalledWith('clean');
  });

  it('says bytes were moved to the Recycle Bin, never that space was freed', async () => {
    const user = userEvent.setup();
    ipc.runBoost.mockResolvedValue(
      boostResult({ movedToBin: { files: 12, bytes: 3.4 * 1024 ** 2, skipped: 2 } }),
    );
    renderHome();
    await user.click(screen.getByRole('button', { name: 'Boost' }));
    const boost = await screen.findByRole('region', { name: 'Boost report' });
    expect(within(boost).getByText(/Moved 12 files \(3\.4 MB\).*to the Recycle Bin/)).toBeTruthy();
    expect(within(boost).getByText(/not freed until the Recycle Bin is emptied/)).toBeTruthy();
    expect(within(boost).queryByText(/space (was|is) freed/i)).toBeNull();
  });

  it('shows the error and keeps the button usable when Boost cannot run', async () => {
    const user = userEvent.setup();
    ipc.runBoost.mockRejectedValue(new IpcUnavailableError());
    renderHome();
    await user.click(screen.getByRole('button', { name: 'Boost' }));
    expect((await screen.findByRole('alert')).textContent).toContain('only available inside');
    expect(screen.queryByRole('region', { name: 'Boost report' })).toBeNull();
    expect(screen.getByRole('button', { name: 'Boost' }).hasAttribute('disabled')).toBe(false);
  });

  it('shows the report of a Boost the tray ran', async () => {
    render(<HomeScreen onNavigate={vi.fn()} trayReport={boostResult()} />);
    expect(await screen.findByRole('region', { name: 'Boost report' })).toBeTruthy();
    expect(ipc.runBoost).not.toHaveBeenCalled();
  });
});
