import { beforeEach, describe, expect, it, vi } from 'vitest';
import { getHealthReport, type HealthView, IpcCommandError, IpcUnavailableError } from './index';

// Mock only the Tauri runtime, so the generated bindings and our wrapper run for real.
const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke, Channel: class {} }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(), once: vi.fn(), emit: vi.fn() }));

beforeEach(() => {
  invoke.mockReset();
});

const view: HealthView = {
  score: 90,
  level: 'good',
  deductions: [
    {
      id: 'restart.pending',
      points: 10,
      title: 'A restart is waiting',
      reason: 'Windows has updates or installs that finish only after a restart.',
      fix: 'restartWindows',
    },
  ],
  unchecked: [],
  systemMount: 'C:\\',
  checkedAt: '2026-11-10T08:30:00.000Z',
};

describe('health report through the generated bindings', () => {
  it('calls the command and returns the typed report', async () => {
    invoke.mockResolvedValue(view);
    await expect(getHealthReport()).resolves.toEqual(view);
    expect(invoke).toHaveBeenCalledWith('health_report');
  });

  it('turns a typed error into an IpcCommandError', async () => {
    invoke.mockRejectedValue({ kind: 'other', message: 'the background task failed' });
    const failure = await getHealthReport().catch((error: unknown) => error);
    expect(failure).toBeInstanceOf(IpcCommandError);
  });

  it('says it is unavailable outside the Volt window', async () => {
    invoke.mockImplementation(() => {
      throw new TypeError("Cannot read properties of undefined (reading 'invoke')");
    });
    await expect(getHealthReport()).rejects.toBeInstanceOf(IpcUnavailableError);
  });
});
