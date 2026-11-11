import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { HealthView, MetricsView } from '../../shared/ipc';
import { IpcCommandError } from '../../shared/ipc';
import { isTrayPanel } from './PanelApp';
import { TrayPanel } from './TrayPanel';

const GIB = 1024 ** 3;

const ipc = vi.hoisted(() => ({
  getHealthReport: vi.fn(),
  getLatestMetrics: vi.fn(),
  onMetricsUpdated: vi.fn(),
  runPanelBoost: vi.fn(),
  closeTrayPanel: vi.fn(),
  openMainFromPanel: vi.fn(),
}));
vi.mock('../../shared/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../shared/ipc')>()),
  ...ipc,
}));

let listeners: Array<(metrics: MetricsView) => void> = [];

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
    disks: [],
    gpu: null,
    ...over,
  };
}

const health: HealthView = {
  score: 82,
  level: 'good',
  deductions: [],
  unchecked: [],
  systemMount: 'C:\\',
  checkedAt: '2026-11-10T08:30:00.000Z',
};

beforeEach(() => {
  listeners = [];
  for (const fn of Object.values(ipc)) fn.mockReset();
  ipc.onMetricsUpdated.mockImplementation(async (handler: (metrics: MetricsView) => void) => {
    listeners.push(handler);
    return () => undefined;
  });
  ipc.getLatestMetrics.mockResolvedValue(reading());
  ipc.getHealthReport.mockResolvedValue(health);
  ipc.closeTrayPanel.mockResolvedValue(undefined);
  ipc.openMainFromPanel.mockResolvedValue(undefined);
});

describe('the tray panel', () => {
  it('is chosen by the view in the address', () => {
    expect(isTrayPanel('?view=tray-panel')).toBe(true);
    expect(isTrayPanel('?section=home')).toBe(false);
    expect(isTrayPanel('')).toBe(false);
  });

  it('shows memory, CPU and the health score', async () => {
    render(<TrayPanel />);
    expect(await screen.findByText('62%')).toBeTruthy();
    expect(screen.getByText('23%')).toBeTruthy();
    expect(await screen.findByText('82 · Good')).toBeTruthy();
  });

  it('follows the live readings', async () => {
    render(<TrayPanel />);
    await screen.findByText('23%');
    act(() => {
      for (const listener of listeners) listener(reading({ cpuPercent: 55 }));
    });
    expect(screen.getByText('55%')).toBeTruthy();
  });

  it('leaves the health score out when the check fails', async () => {
    ipc.getHealthReport.mockRejectedValue(new Error('no'));
    render(<TrayPanel />);
    await screen.findByText('62%');
    expect(screen.queryByText('Health score')).toBeNull();
  });

  it('runs Boost once and shows the line the shell wrote', async () => {
    let finish: (value: { line: string; failed: boolean }) => void = () => undefined;
    ipc.runPanelBoost.mockReturnValue(
      new Promise((resolve) => {
        finish = resolve;
      }),
    );
    const user = userEvent.setup();
    render(<TrayPanel />);
    await screen.findByText('62%');
    await user.click(screen.getByRole('button', { name: 'Boost' }));
    await user.click(screen.getByRole('button', { name: 'Boosting...' }));
    expect(ipc.runPanelBoost).toHaveBeenCalledTimes(1);
    finish({ line: 'Available memory went from 4 GB to 5 GB.', failed: false });
    expect(await screen.findByText('Available memory went from 4 GB to 5 GB.')).toBeTruthy();
  });

  it('says so when another Boost is running', async () => {
    ipc.runPanelBoost.mockResolvedValue(null);
    const user = userEvent.setup();
    render(<TrayPanel />);
    await user.click(screen.getByRole('button', { name: 'Boost' }));
    expect(await screen.findByText('A Boost is already running.')).toBeTruthy();
  });

  it('shows why Boost could not start', async () => {
    ipc.runPanelBoost.mockRejectedValue(
      new IpcCommandError({ kind: 'other', message: 'The engine is busy.' }),
    );
    const user = userEvent.setup();
    render(<TrayPanel />);
    await user.click(screen.getByRole('button', { name: 'Boost' }));
    expect(await screen.findByText(/The engine is busy\./)).toBeTruthy();
  });

  it('closes on Escape', async () => {
    const user = userEvent.setup();
    render(<TrayPanel />);
    await user.keyboard('{Escape}');
    await waitFor(() => expect(ipc.closeTrayPanel).toHaveBeenCalledTimes(1));
  });

  it('opens the full window from its button', async () => {
    const user = userEvent.setup();
    render(<TrayPanel />);
    await user.click(screen.getByRole('button', { name: 'Open Volt' }));
    expect(ipc.openMainFromPanel).toHaveBeenCalledTimes(1);
  });
});
