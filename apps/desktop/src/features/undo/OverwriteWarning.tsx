import { t, tn } from '../../shared/i18n';
import type { UndoOverwrite } from '../../shared/ipc';
import { formatTime, sourceLabel } from './messages';

/**
 * Warns that an undo would replace a later change to the same setting, and names that change. The
 * core decides what is overwritten; this only shows it.
 */
export function OverwriteWarning({ overwrites }: { overwrites: UndoOverwrite[] }) {
  if (overwrites.length === 0) return null;
  return (
    <div role="alert" className="mb-3 rounded-md bg-caution p-3 text-body text-caution-fg">
      <p className="font-semibold">{tn('undo.overwrite.title', overwrites.length)}</p>
      <ul aria-label={t('undo.overwrite.label')} className="mt-1 space-y-1">
        {overwrites.map((overwrite) => (
          <li key={`${overwrite.entry}:${overwrite.laterEntry}`} className="break-words">
            {t('undo.overwrite.line', {
              what: overwrite.what,
              time: formatTime(overwrite.laterTime),
              source: sourceLabel(overwrite.laterSource),
            })}
          </li>
        ))}
      </ul>
    </div>
  );
}
