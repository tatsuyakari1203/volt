import { useRef } from 'react';
import { t } from '../../shared/i18n';
import type { UndoPreview } from '../../shared/ipc';
import { Button, Dialog } from '../../shared/ui';
import { OverwriteWarning } from './OverwriteWarning';

/** A single change to confirm, with what undoing it would do. */
export interface EntryTarget {
  /** The journal entry id, a decimal string. */
  entry: string;
  /** What undoing it would restore and overwrite. */
  preview: UndoPreview;
}

export interface EntryUndoDialogProps {
  /** The change to confirm, or `null` while closed. */
  target: EntryTarget | null;
  /** Called on Cancel, Escape or a backdrop click. */
  onClose: () => void;
  /** Called when the user confirms. The dialog does not close itself. */
  onConfirm: (target: EntryTarget) => void;
}

/** Confirms undoing one change that would overwrite a later change. */
export function EntryUndoDialog({ target, onClose, onConfirm }: EntryUndoDialogProps) {
  const cancelRef = useRef<HTMLButtonElement>(null);
  return (
    <Dialog
      open={target !== null}
      onClose={onClose}
      title={t('undo.entry.title')}
      description={t('undo.entry.description')}
      initialFocusRef={cancelRef}
      footer={
        <>
          <Button ref={cancelRef} onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button variant="primary" onClick={() => target && onConfirm(target)}>
            {t('undo.entry.confirm')}
          </Button>
        </>
      }
    >
      {target ? (
        <>
          <OverwriteWarning overwrites={target.preview.overwrites} />
          <ul aria-label={t('undo.restore.label')} className="space-y-2">
            {target.preview.items.map((item) => (
              <li key={item.entry} className="text-body">
                <span className="block break-words font-semibold">{item.what}</span>
                <span className="block text-muted">
                  {t('undo.restore.will', { restore: item.restore })}
                </span>
              </li>
            ))}
          </ul>
        </>
      ) : null}
    </Dialog>
  );
}
