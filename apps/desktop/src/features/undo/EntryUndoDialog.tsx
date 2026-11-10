import { useRef } from 'react';
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
      title="Undo this change?"
      description="Volt will put back the value it saved before it made this change."
      initialFocusRef={cancelRef}
      footer={
        <>
          <Button ref={cancelRef} onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" onClick={() => target && onConfirm(target)}>
            Undo anyway
          </Button>
        </>
      }
    >
      {target ? (
        <>
          <OverwriteWarning overwrites={target.preview.overwrites} />
          <ul aria-label="What will be restored" className="space-y-2">
            {target.preview.items.map((item) => (
              <li key={item.entry} className="text-body">
                <span className="block break-words font-semibold">{item.what}</span>
                <span className="block text-muted">Will be {item.restore}.</span>
              </li>
            ))}
          </ul>
        </>
      ) : null}
    </Dialog>
  );
}
