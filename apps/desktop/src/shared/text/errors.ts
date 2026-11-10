import { t } from '../i18n';
import { describeIpcError, IpcCommandError, IpcUnavailableError } from '../ipc';

/** One sentence for any error that reached the UI. */
export function describeError(error: unknown): string {
  if (error instanceof IpcUnavailableError) {
    return t('error.unavailable');
  }
  if (error instanceof IpcCommandError) {
    return describeIpcError(error.error);
  }
  return error instanceof Error ? error.message : String(error);
}
