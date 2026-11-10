import type { UndoOverwrite } from '../../shared/ipc';
import { formatTime, sourceLabels } from './messages';

/**
 * Warns that an undo would replace a later change to the same setting, and names that change. The
 * core decides what is overwritten; this only shows it.
 */
export function OverwriteWarning({ overwrites }: { overwrites: UndoOverwrite[] }) {
  if (overwrites.length === 0) return null;
  return (
    <div role="alert" className="mb-3 rounded-md bg-caution p-3 text-body text-caution-fg">
      <p className="font-semibold">
        {overwrites.length === 1
          ? 'A later change will be overwritten'
          : 'Later changes will be overwritten'}
      </p>
      <ul aria-label="Later changes that will be overwritten" className="mt-1 space-y-1">
        {overwrites.map((overwrite) => (
          <li key={`${overwrite.entry}:${overwrite.laterEntry}`} className="break-words">
            {overwrite.what} was changed again on {formatTime(overwrite.laterTime)} (
            {sourceLabels[overwrite.laterSource]}). Undoing the older change replaces it.
          </li>
        ))}
      </ul>
    </div>
  );
}
