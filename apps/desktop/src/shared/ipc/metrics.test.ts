import { beforeEach, describe, expect, it, vi } from 'vitest';
import { getLatestMetrics, IpcUnavailableError, type MetricsView, onMetricsUpdated } from './index';

// Mock only the Tauri runtime, so the generated bindings and our wrapper run for real.
const { invoke, listen } = vi.hoisted(() => ({ invoke: vi.fn(), listen: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke, Channel: class {} }));
vi.mock('@tauri-apps/api/event', () => ({ listen, once: vi.fn(), emit: vi.fn() }));

beforeEach(() => {
  invoke.mockReset();
  listen.mockReset();
});

/** A reading of a machine without a GPU: every other metric is present, `gpu` is null. */
const withoutGpu: MetricsView = {
  takenAt: '2026-11-10T03:01:00.000Z',
  cpuPercent: 12.5,
  memory: { totalBytes: 16e9, usedBytes: 8e9, availableBytes: 8e9, usedPercent: 50 },
  disks: [{ mount: 'C:\\', totalBytes: 5e11, usedBytes: 3e11, freeBytes: 2e11, usedPercent: 60 }],
  gpu: null,
};

describe('live metrics through the generated bindings', () => {
  it('listens to the typed event and hands the payload to the handler', async () => {
    const unlisten = vi.fn();
    listen.mockResolvedValue(unlisten);
    const handler = vi.fn();

    const stop = await onMetricsUpdated(handler);
    expect(listen).toHaveBeenCalledWith('metrics-updated', expect.any(Function));
    const callback = listen.mock.calls[0]?.[1] as (event: { payload: unknown }) => void;
    callback({ payload: withoutGpu });
    expect(handler).toHaveBeenCalledWith(withoutGpu);
    expect(stop).toBe(unlisten);
  });

  it('reads the latest reading, which has no GPU on a machine without one', async () => {
    invoke.mockResolvedValue(withoutGpu);
    const metrics = await getLatestMetrics();
    expect(invoke).toHaveBeenCalledWith('latest_metrics');
    expect(metrics?.gpu).toBeNull();
    expect(metrics?.memory.usedPercent).toBe(50);
  });

  it('resolves with null before the first reading of a window', async () => {
    invoke.mockResolvedValue(null);
    await expect(getLatestMetrics()).resolves.toBeNull();
  });

  it('says it is unavailable outside the Volt window', async () => {
    invoke.mockRejectedValue(new TypeError('no bridge'));
    await expect(getLatestMetrics()).rejects.toBeInstanceOf(IpcUnavailableError);
  });
});
