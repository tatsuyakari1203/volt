import { beforeEach, describe, expect, it, vi } from 'vitest';
import { commands } from './bindings';
import {
  describeIpcError,
  getAppInfo,
  getAutostartEnabled,
  IpcCommandError,
  type IpcError,
  onAppReady,
  onTrayBoostRequested,
  setAutostartEnabled,
} from './index';

// Mock only the Tauri runtime, so the generated bindings and our wrapper run for real.
const { invoke, listen } = vi.hoisted(() => ({ invoke: vi.fn(), listen: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke, Channel: class {} }));
vi.mock('@tauri-apps/api/event', () => ({ listen, once: vi.fn(), emit: vi.fn() }));

beforeEach(() => {
  invoke.mockReset();
  listen.mockReset();
});

describe('app_info, end to end through the generated bindings', () => {
  it('returns the data the Rust command resolved with', async () => {
    invoke.mockResolvedValue({ name: 'Volt', version: '1.2.3' });
    await expect(getAppInfo()).resolves.toEqual({ name: 'Volt', version: '1.2.3' });
    expect(invoke).toHaveBeenCalledWith('app_info');
  });

  it('hands the UI a discriminated error value that narrows on kind', async () => {
    invoke.mockRejectedValue({ kind: 'accessDenied', message: 'needs administrator' });
    const result = await commands.appInfo();
    if (result.status !== 'error') {
      throw new Error('expected an error result');
    }
    const error: IpcError = result.error;
    switch (error.kind) {
      case 'accessDenied': {
        // Narrowed: `message` is reachable and no other variant's shape is assumed.
        const message: string = error.message;
        expect(message).toBe('needs administrator');
        return;
      }
      default:
        throw new Error(`unexpected kind ${error.kind}`);
    }
  });

  it('getAppInfo rejects with an IpcCommandError that keeps the typed error', async () => {
    invoke.mockRejectedValue({ kind: 'notFound', message: 'run key' });
    const failure = await getAppInfo().catch((e: unknown) => e);
    expect(failure).toBeInstanceOf(IpcCommandError);
    if (failure instanceof IpcCommandError) {
      expect(failure.error).toEqual({ kind: 'notFound', message: 'run key' });
      expect(failure.message).toBe('Not found: run key');
    }
  });
});

describe('describeIpcError', () => {
  it.each<[IpcError, string]>([
    [{ kind: 'notFound', message: 'a' }, 'Not found: a'],
    [{ kind: 'accessDenied', message: 'b' }, 'Access denied: b'],
    [{ kind: 'unsupported', message: 'c' }, 'Not supported: c'],
    [{ kind: 'invalidInput', message: 'd' }, 'Invalid input: d'],
    [{ kind: 'other', message: 'e' }, 'e'],
  ])('describes %j', (error, text) => {
    expect(describeIpcError(error)).toBe(text);
  });
});

describe('app-ready event', () => {
  it('passes the typed payload to the handler and subscribes to the generated event name', async () => {
    const unlisten = vi.fn();
    listen.mockResolvedValue(unlisten);
    const handler = vi.fn();

    const stop = await onAppReady(handler);
    expect(listen).toHaveBeenCalledWith('app-ready', expect.any(Function));

    const callback = listen.mock.calls[0]?.[1] as (event: { payload: unknown }) => void;
    callback({ payload: { name: 'Volt', version: '1.2.3' } });
    expect(handler).toHaveBeenCalledWith({ name: 'Volt', version: '1.2.3' });
    expect(stop).toBe(unlisten);
  });
});

describe('autostart commands', () => {
  it('reads the state through the generated binding', async () => {
    invoke.mockResolvedValue(true);
    await expect(getAutostartEnabled()).resolves.toBe(true);
    expect(invoke).toHaveBeenCalledWith('autostart_enabled');
  });

  it('sets the state and resolves with what the shell reports back', async () => {
    invoke.mockResolvedValue(false);
    await expect(setAutostartEnabled(true)).resolves.toBe(false);
    expect(invoke).toHaveBeenCalledWith('set_autostart', { enabled: true });
  });

  it('rejects with the typed error when the shell cannot change it', async () => {
    invoke.mockRejectedValue({ kind: 'other', message: 'registry unavailable' });
    const failure = await setAutostartEnabled(true).catch((e: unknown) => e);
    expect(failure).toBeInstanceOf(IpcCommandError);
  });
});

describe('tray boost event', () => {
  it('subscribes to the generated event name and calls the handler', async () => {
    const unlisten = vi.fn();
    listen.mockResolvedValue(unlisten);
    const handler = vi.fn();

    const stop = await onTrayBoostRequested(handler);
    expect(listen).toHaveBeenCalledWith('tray-boost-requested', expect.any(Function));
    const callback = listen.mock.calls[0]?.[1] as (event: { payload: unknown }) => void;
    callback({ payload: null });
    expect(handler).toHaveBeenCalledTimes(1);
    expect(stop).toBe(unlisten);
  });
});
