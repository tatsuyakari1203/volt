import { useRef } from 'react';
import { Button, Dialog } from '../../shared/ui';

export interface ClearHistoryDialogProps {
  open: boolean;
  /** True while the command runs. */
  busy: boolean;
  onClose: () => void;
  onConfirm: () => void;
}

/** Confirms clearing the history: says what goes and that changes that can be undone stay. */
export function ClearHistoryDialog({ open, busy, onClose, onConfirm }: ClearHistoryDialogProps) {
  const cancelRef = useRef<HTMLButtonElement>(null);
  return (
    <Dialog
      open={open}
      onClose={onClose}
      title="Clear the history?"
      description="The records of past changes are removed from this list. This cannot be undone."
      initialFocusRef={cancelRef}
      footer={
        <>
          <Button ref={cancelRef} onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" loading={busy} onClick={onConfirm}>
            Clear history
          </Button>
        </>
      }
    >
      <p className="text-body text-muted">
        Changes that can still be undone stay in the list, and nothing on your PC is changed. Volt
        also removes very old records by itself.
      </p>
    </Dialog>
  );
}
