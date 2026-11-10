import { useEffect, useRef, useState } from 'react';
import { previewUndoBatch, type UndoPreviewItem } from '../../shared/ipc';
import { Button, Dialog } from '../../shared/ui';
import { describeError } from './messages';

/** The batch the user asked to undo. */
export interface BatchTarget {
  /** The journal batch id, a decimal string. */
  batch: string;
  /** What to call it: the tweak title. */
  title: string;
}

type Preview =
  | { status: 'loading' }
  | { status: 'error'; message: string }
  | { status: 'ready'; items: UndoPreviewItem[] };

export interface BatchUndoDialogProps {
  /** The batch to confirm, or `null` while closed. */
  target: BatchTarget | null;
  /** Called on Cancel, Escape or a backdrop click. */
  onClose: () => void;
  /** Called when the user confirms. The dialog does not close itself. */
  onConfirm: (target: BatchTarget) => void;
}

function confirmLabel(count: number): string {
  if (count === 0) return 'Undo';
  return count === 1 ? 'Undo 1 change' : `Undo ${count} changes`;
}

/**
 * Confirms undoing a whole batch. It reads what is still applied in the batch from the journal
 * and lists it, so the user sees exactly which values will be put back before agreeing.
 */
export function BatchUndoDialog({ target, onClose, onConfirm }: BatchUndoDialogProps) {
  const [preview, setPreview] = useState<Preview>({ status: 'loading' });
  const cancelRef = useRef<HTMLButtonElement>(null);
  const batch = target?.batch ?? null;

  useEffect(() => {
    if (batch === null) return;
    let current = true;
    setPreview({ status: 'loading' });
    previewUndoBatch(batch)
      .then((result) => {
        if (current) setPreview({ status: 'ready', items: result.items });
      })
      .catch((error: unknown) => {
        if (current) setPreview({ status: 'error', message: describeError(error) });
      });
    return () => {
      current = false;
    };
  }, [batch]);

  const items = preview.status === 'ready' ? preview.items : [];
  return (
    <Dialog
      open={target !== null}
      onClose={onClose}
      title="Undo all of these changes?"
      description={
        target
          ? `Volt will put back the values it saved before it changed "${target.title}".`
          : undefined
      }
      initialFocusRef={cancelRef}
      footer={
        <>
          <Button ref={cancelRef} onClick={onClose}>
            Cancel
          </Button>
          <Button
            variant="primary"
            disabled={items.length === 0}
            onClick={() => target && onConfirm(target)}
          >
            {confirmLabel(items.length)}
          </Button>
        </>
      }
    >
      {preview.status === 'loading' ? (
        <p role="status" className="text-body text-muted">
          Reading what will be restored...
        </p>
      ) : null}
      {preview.status === 'error' ? (
        <p role="alert" className="text-body text-muted">
          Could not read what would be restored. {preview.message}
        </p>
      ) : null}
      {preview.status === 'ready' && items.length === 0 ? (
        <p className="text-body text-muted">
          Nothing is left to undo in this batch. It was already undone.
        </p>
      ) : null}
      {items.length > 0 ? (
        <ul aria-label="What will be restored" className="max-h-60 space-y-2 overflow-y-auto">
          {items.map((item) => (
            <li key={item.entry} className="text-body">
              <span className="block break-words font-semibold">{item.what}</span>
              <span className="block text-muted">Will be {item.restore}.</span>
            </li>
          ))}
        </ul>
      ) : null}
    </Dialog>
  );
}
