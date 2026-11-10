// The frontend's only door to the Rust shell. Commands, events and transfer types come from
// `bindings.ts`, which is generated from Rust (see docs/architecture.md): never hand-write a
// transfer type or call `invoke` directly. This file adds ergonomics on top of the generated
// module and nothing else.
import {
  type AppInfo,
  type AppReady,
  commands,
  events,
  type IpcError,
  type Result,
} from './bindings';

export type { AppInfo, AppReady, IpcError };

/** Thrown by the wrappers below when a command returns an error; carries the typed value. */
export class IpcCommandError extends Error {
  /** The typed error the Rust command returned. Switch on `error.kind`. */
  readonly error: IpcError;

  constructor(error: IpcError) {
    super(describeIpcError(error));
    this.name = 'IpcCommandError';
    this.error = error;
  }
}

/** One-line text for an IPC error. The switch is exhaustive: a new Rust variant fails typecheck. */
export function describeIpcError(error: IpcError): string {
  switch (error.kind) {
    case 'notFound':
      return `Not found: ${error.message}`;
    case 'accessDenied':
      return `Access denied: ${error.message}`;
    case 'unsupported':
      return `Not supported: ${error.message}`;
    case 'invalidInput':
      return `Invalid input: ${error.message}`;
    case 'other':
      return error.message;
    default:
      return assertNever(error);
  }
}

function assertNever(value: never): never {
  throw new Error(`Unhandled IPC error: ${JSON.stringify(value)}`);
}

/** Returns the data of a command result, or throws an `IpcCommandError` carrying the error. */
export function unwrap<T>(result: Result<T, IpcError>): T {
  if (result.status === 'error') {
    throw new IpcCommandError(result.error);
  }
  return result.data;
}

/** Asks the Rust side who it is. Proves the frontend-to-Rust path works. */
export async function getAppInfo(): Promise<AppInfo> {
  return unwrap(await commands.appInfo());
}

/** Calls `handler` each time the shell announces that the app is ready. Returns the unlisten function. */
export function onAppReady(handler: (info: AppReady) => void): Promise<() => void> {
  return events.appReady.listen((event) => handler(event.payload));
}

/** Whether Volt is registered to start with Windows (per-user, unelevated). */
export async function getAutostartEnabled(): Promise<boolean> {
  return unwrap(await commands.autostartEnabled());
}

/** Turns start with Windows on or off. Resolves with the state that is true afterwards. */
export async function setAutostartEnabled(enabled: boolean): Promise<boolean> {
  return unwrap(await commands.setAutostart(enabled));
}

/**
 * Calls `handler` each time the user picks Boost in the tray menu. Boost has no engine yet, so
 * today the shell only brings the window to Home; a later task starts the real work here.
 * Returns the unlisten function.
 */
export function onTrayBoostRequested(handler: () => void): Promise<() => void> {
  return events.trayBoostRequested.listen(() => handler());
}
