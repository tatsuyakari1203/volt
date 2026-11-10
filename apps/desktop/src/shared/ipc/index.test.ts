import { beforeEach, describe, expect, it, vi } from 'vitest';
import { commands } from './bindings';
import {
  applyTweak,
  describeIpcError,
  exportActivity,
  exportActivityText,
  getAppInfo,
  getAutostartEnabled,
  IpcCommandError,
  type IpcError,
  IpcUnavailableError,
  listActivity,
  listTweaks,
  onAppReady,
  onTrayBoostRequested,
  previewUndoBatch,
  revertTweak,
  setAutostartEnabled,
  undoBatch,
  undoEntry,
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

describe('tweak and activity commands', () => {
  it('lists tweaks through the generated binding', async () => {
    invoke.mockResolvedValue([{ id: 'show-file-extensions' }]);
    await expect(listTweaks()).resolves.toEqual([{ id: 'show-file-extensions' }]);
    expect(invoke).toHaveBeenCalledWith('list_tweaks');
  });

  it('applies and reverts by id and resolves with the precise result', async () => {
    invoke.mockResolvedValueOnce({ status: 'applied' });
    await expect(applyTweak('t')).resolves.toEqual({ status: 'applied' });
    expect(invoke).toHaveBeenLastCalledWith('apply_tweak', { id: 't' });

    invoke.mockResolvedValueOnce({ status: 'reverted' });
    await expect(revertTweak('t')).resolves.toEqual({ status: 'reverted' });
    expect(invoke).toHaveBeenLastCalledWith('revert_tweak', { id: 't' });
  });

  it('reads the activity with a limit and an optional cursor', async () => {
    invoke.mockResolvedValue({ items: [], nextCursor: null });
    await listActivity(50);
    expect(invoke).toHaveBeenLastCalledWith('list_activity', { limit: 50, cursor: null });
    await listActivity(10, '42');
    expect(invoke).toHaveBeenLastCalledWith('list_activity', { limit: 10, cursor: '42' });
  });

  it('undoes, previews and exports through the generated commands with string ids', async () => {
    invoke.mockResolvedValue({ status: 'restored', restored: 1, failed: 0, failures: [] });
    await expect(undoEntry('7')).resolves.toMatchObject({ status: 'restored' });
    expect(invoke).toHaveBeenLastCalledWith('undo_entry', { entry: '7' });
    await undoBatch('3');
    expect(invoke).toHaveBeenLastCalledWith('undo_batch', { batch: '3' });
    invoke.mockResolvedValue({ items: [] });
    await previewUndoBatch('3');
    expect(invoke).toHaveBeenLastCalledWith('preview_undo_batch', { batch: '3' });
    invoke.mockResolvedValue({ kind: 'cancelled' });
    await expect(exportActivity()).resolves.toEqual({ kind: 'cancelled' });
    expect(invoke).toHaveBeenLastCalledWith('export_activity');
    invoke.mockResolvedValue({ fileName: 'a.txt', text: '', records: 0 });
    await exportActivityText();
    expect(invoke).toHaveBeenLastCalledWith('export_activity_text');
  });

  it('turns a typed error into an IpcCommandError', async () => {
    invoke.mockRejectedValue({ kind: 'notFound', message: 'unknown tweak `x`' });
    const failure = await applyTweak('x').catch((e: unknown) => e);
    expect(failure).toBeInstanceOf(IpcCommandError);
  });
});

describe('outside the Volt window', () => {
  it('turns the missing bridge into an IpcUnavailableError, not a raw TypeError', async () => {
    invoke.mockImplementation(() => {
      throw new TypeError("Cannot read properties of undefined (reading 'invoke')");
    });
    for (const call of [
      () => getAppInfo(),
      () => listTweaks(),
      () => applyTweak('t'),
      () => revertTweak('t'),
      () => listActivity(10),
      () => undoEntry('1'),
      () => undoBatch('1'),
      () => previewUndoBatch('1'),
      () => exportActivity(),
      () => getAutostartEnabled(),
    ]) {
      await expect(call()).rejects.toBeInstanceOf(IpcUnavailableError);
    }
  });

  it('does not hide a TypeError raised inside the real window', async () => {
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    try {
      invoke.mockImplementation(() => {
        throw new TypeError('a real bug');
      });
      await expect(listTweaks()).rejects.toThrow('a real bug');
    } finally {
      Reflect.deleteProperty(window, '__TAURI_INTERNALS__');
    }
  });
});
