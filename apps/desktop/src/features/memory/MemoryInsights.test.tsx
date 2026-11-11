import { act, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type {
  MemoryPointView,
  MemoryState,
  ProcessGroupView,
  ProcessListView,
} from '../../shared/ipc';
import { IpcCommandError } from '../../shared/ipc';
import { describeArea } from '../../shared/text/memory';
import { ExclusionsCard } from './ExclusionsCard';
import { MemoryChart } from './MemoryChart';
import { MemoryScreen } from './MemoryScreen';
import { buildLayers, coveredMinutes, MAX_POINTS, mergePoints, SPAN_MS } from './trend';

const ipc = vi.hoisted(() => ({
  getMemoryState: vi.fn(),
  getElevationStatus: vi.fn(),
  getMemoryHistory: vi.fn(),
  onMemoryTrend: vi.fn(),
  getMemoryExclusions: vi.fn(),
  addMemoryExclusion: vi.fn(),
  removeMemoryExclusion: vi.fn(),
  listProcesses: vi.fn(),
}));
vi.mock('../../shared/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../shared/ipc')>()),
  ...ipc,
}));

const GB = 1024 ** 3;
const MB = 1024 ** 2;
const START = Date.parse('2026-11-11T10:00:00.000Z');

function point(second: number, inUse = 8, standby = 4, modified = 1): MemoryPointView {
  return {
    takenAt: new Date(START + second * 1000).toISOString(),
    lists: {
      totalBytes: 16 * GB,
      inUseBytes: inUse * GB,
      standbyBytes: standby * GB,
      modifiedBytes: modified * GB,
      freeBytes: (16 - inUse - standby - modified) * GB,
    },
  };
}

function series(count: number): MemoryPointView[] {
  return Array.from({ length: count }, (_, index) => point(index, 8 + (index % 3)));
}

beforeEach(() => {
  for (const mock of Object.values(ipc)) mock.mockReset();
  ipc.onMemoryTrend.mockResolvedValue(() => {});
  ipc.getMemoryHistory.mockResolvedValue([]);
  ipc.getMemoryExclusions.mockResolvedValue([]);
  ipc.getElevationStatus.mockResolvedValue({ state: 'available' });
});

describe('the chart geometry', () => {
  it('draws nothing for fewer than two readings', () => {
    expect(buildLayers([], 300, 100)).toEqual([]);
    expect(buildLayers([point(0)], 300, 100)).toEqual([]);
  });

  it('stacks the four lists from the bottom to the installed memory', () => {
    const layers = buildLayers([point(0), point(1)], 300, 100);
    expect(layers.map((layer) => layer.key)).toEqual([
      'inUseBytes',
      'standbyBytes',
      'modifiedBytes',
      'freeBytes',
    ]);
    // In use is 8 of 16 GB: its top edge is at half the height. Free ends at the top (y = 0).
    expect(layers[0]?.edge).toMatch(/,50 L/);
    expect(layers[3]?.edge).toMatch(/,0 L/);
  });

  it('puts the newest reading at the right edge and spans five minutes', () => {
    const layers = buildLayers([point(0), point(SPAN_MS / 2000)], 300, 100);
    // Half the span ago is x = 150; now is x = 300.
    expect(layers[0]?.edge.startsWith('M150,')).toBe(true);
    expect(layers[0]?.edge).toMatch(/ L300,/);
  });

  it('merges readings by time without duplicates and keeps the newest ones', () => {
    const merged = mergePoints([point(2), point(1)], [point(2), point(3)]);
    expect(merged.map((p) => p.takenAt)).toEqual(
      [point(1), point(2), point(3)].map((p) => p.takenAt),
    );
    const many = mergePoints([], series(MAX_POINTS + 20));
    expect(many).toHaveLength(MAX_POINTS);
    expect(many[0]?.takenAt).toBe(point(20).takenAt);
  });

  it('counts the minutes the readings cover, at least one', () => {
    expect(coveredMinutes([])).toBe(0);
    expect(coveredMinutes([point(0), point(5)])).toBe(1);
    expect(coveredMinutes([point(0), point(185)])).toBe(3);
  });
});

describe('MemoryChart', () => {
  it('says it is collecting readings until there are two', () => {
    render(<MemoryChart points={[point(0)]} />);
    expect(screen.getByRole('status').textContent).toMatch(/Collecting readings/);
  });

  it('has a text alternative with the numbers, and a table for screen readers', () => {
    render(<MemoryChart points={series(90)} />);
    const picture = screen.getByRole('img', { name: /Memory by list over the last 1 minute/ });
    expect(picture.getAttribute('aria-label')).toMatch(/in use 10 GB, standby 4 GB/);
    const table = screen.getByRole('table');
    expect(within(table).getByRole('columnheader', { name: 'Lowest' })).toBeTruthy();
    expect(within(table).getByRole('row', { name: /In use 10 GB 8 GB 10 GB/ })).toBeTruthy();
  });

  it('is a still picture: nothing animates, so reduced motion needs nothing', () => {
    const { container } = render(<MemoryChart points={series(30)} />);
    expect(container.innerHTML).not.toMatch(/animate-|transition/);
    // Every layer has an outline, so the stack is not told apart by colour alone.
    expect(container.querySelectorAll('svg path[fill="none"]')).toHaveLength(4);
  });
});

describe('the chart on the Memory page', () => {
  it('loads the history, follows new readings and stops listening on leave', async () => {
    let push: (next: MemoryPointView) => void = () => {};
    const unlisten = vi.fn();
    ipc.onMemoryTrend.mockImplementation(async (handler: (p: MemoryPointView) => void) => {
      push = handler;
      return unlisten;
    });
    ipc.getMemoryHistory.mockResolvedValue(series(60));
    ipc.getMemoryState.mockResolvedValue({
      lists: point(0).lists,
      problem: null,
      operations: [],
    } satisfies MemoryState);
    const { unmount } = render(<MemoryScreen />);

    const chart = await screen.findByRole('region', { name: 'Memory over time' });
    await waitFor(() => expect(within(chart).getByRole('img')).toBeTruthy());
    expect(within(chart).getByRole('img').getAttribute('aria-label')).toMatch(/last 1 minute\b/);

    act(() => push(point(200)));
    await waitFor(() =>
      expect(within(chart).getByRole('img').getAttribute('aria-label')).toMatch(/last 3 minutes/),
    );
    unmount();
    expect(unlisten).toHaveBeenCalled();
  });

  it('keeps the page working when the history cannot be read', async () => {
    ipc.getMemoryHistory.mockRejectedValue(new Error('no shell'));
    ipc.getMemoryState.mockResolvedValue({
      lists: point(0).lists,
      problem: null,
      operations: [],
    } satisfies MemoryState);
    render(<MemoryScreen />);
    expect(await screen.findByText(/Collecting readings/)).toBeTruthy();
  });
});

describe('ExclusionsCard', () => {
  const group = (name: string, memoryBytes: number): ProcessGroupView => ({
    id: name,
    name,
    path: null,
    processCount: 1,
    endCount: 1,
    memoryBytes,
    cpuPercent: 0,
    endability: 'allowed',
  });
  const running: ProcessListView = {
    groups: [group('Game.exe', 900 * MB), group('notes.exe', 40 * MB), group('db.exe', 300 * MB)],
    processCount: 3,
    logicalCpus: 8,
  };

  it('shows the saved list and removes a program', async () => {
    const user = userEvent.setup();
    ipc.getMemoryExclusions.mockResolvedValue(['db.exe', 'game.exe']);
    ipc.removeMemoryExclusion.mockResolvedValue(['game.exe']);
    render(<ExclusionsCard />);

    const list = await screen.findByRole('list', { name: 'Protected programs list' });
    expect(within(list).getAllByRole('listitem')).toHaveLength(2);
    await user.click(screen.getByRole('button', { name: 'Stop protecting db.exe' }));
    expect(ipc.removeMemoryExclusion).toHaveBeenCalledWith('db.exe');
    await waitFor(() => expect(screen.queryByText('db.exe')).toBeNull());
    expect(screen.getByText('game.exe')).toBeTruthy();
  });

  it('adds a program by its executable name', async () => {
    const user = userEvent.setup();
    ipc.addMemoryExclusion.mockResolvedValue(['game.exe']);
    render(<ExclusionsCard />);
    expect(await screen.findByText('No program is protected.')).toBeTruthy();

    const add = screen.getByRole('button', { name: 'Add' }) as HTMLButtonElement;
    expect(add.disabled).toBe(true);
    await user.type(screen.getByRole('textbox', { name: 'Executable name' }), 'Game');
    await user.click(add);
    expect(ipc.addMemoryExclusion).toHaveBeenCalledWith('Game');
    expect(await screen.findByText('game.exe')).toBeTruthy();
    expect(
      (screen.getByRole('textbox', { name: 'Executable name' }) as HTMLInputElement).value,
    ).toBe('');
  });

  it('says why a name was refused and keeps what was typed', async () => {
    const user = userEvent.setup();
    ipc.addMemoryExclusion.mockRejectedValue(
      new IpcCommandError({ kind: 'invalidInput', message: 'bad name' }),
    );
    render(<ExclusionsCard />);
    await user.type(await screen.findByRole('textbox', { name: 'Executable name' }), 'a|b');
    await user.click(screen.getByRole('button', { name: 'Add' }));
    expect((await screen.findByRole('alert')).textContent).toMatch(/bad name/);
    expect(
      (screen.getByRole('textbox', { name: 'Executable name' }) as HTMLInputElement).value,
    ).toBe('a|b');
  });

  it('picks from the running programs and leaves out the ones already protected', async () => {
    const user = userEvent.setup();
    ipc.getMemoryExclusions.mockResolvedValue(['db.exe']);
    ipc.listProcesses.mockResolvedValue(running);
    ipc.addMemoryExclusion.mockResolvedValue(['db.exe', 'game.exe']);
    render(<ExclusionsCard />);
    await screen.findByRole('list', { name: 'Protected programs list' });
    expect(ipc.listProcesses).not.toHaveBeenCalled();

    await user.click(screen.getByRole('button', { name: 'Choose from running programs' }));
    const picker = await screen.findByRole('list', { name: 'Running programs' });
    const names = within(picker)
      .getAllByRole('listitem')
      .map((item) => item.querySelector('.font-semibold')?.textContent);
    expect(names).toEqual(['Game.exe', 'notes.exe']);

    await user.click(within(picker).getByRole('button', { name: 'Protect Game.exe' }));
    expect(ipc.addMemoryExclusion).toHaveBeenCalledWith('Game.exe');
    await waitFor(() =>
      expect(
        within(screen.getByRole('list', { name: 'Running programs' })).getAllByRole('listitem'),
      ).toHaveLength(1),
    );
  });
});

describe('the report says how many protected programs were left alone', () => {
  const base = {
    area: 'workingSets',
    status: 'done',
    processesTrimmed: 12,
    processesSkipped: 0,
    processesExcluded: 0,
    detail: null,
  } as const;

  it('adds a sentence only when the list protected something', () => {
    expect(describeArea(base)).toBe('12 programs trimmed.');
    expect(describeArea({ ...base, processesExcluded: 1 })).toBe(
      '12 programs trimmed. 1 protected program was left alone.',
    );
    expect(describeArea({ ...base, processesSkipped: 3, processesExcluded: 2 })).toBe(
      '12 programs trimmed, 3 could not be opened and were left alone. 2 protected programs were left alone.',
    );
  });
});
