import { useRef } from 'react';
import { t } from '../../shared/i18n';
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
      title={t('undo.clearDialog.title')}
      description={t('undo.clearDialog.description')}
      initialFocusRef={cancelRef}
      footer={
        <>
          <Button ref={cancelRef} onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button variant="primary" loading={busy} onClick={onConfirm}>
            {t('undo.clearDialog.confirm')}
          </Button>
        </>
      }
    >
      <p className="text-body text-muted">{t('undo.clearDialog.note')}</p>
    </Dialog>
  );
}
