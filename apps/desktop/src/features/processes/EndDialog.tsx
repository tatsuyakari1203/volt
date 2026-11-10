import { TriangleAlert } from 'lucide-react';
import { t, tn } from '../../shared/i18n';
import type { ProcessGroupView } from '../../shared/ipc';
import { Button, Dialog } from '../../shared/ui';
import { processesText } from './processes';

export interface EndDialogProps {
  /** The app to end, or `null` while the dialog is closed. */
  group: ProcessGroupView | null;
  /** Whether the end command is running. */
  ending: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}

/**
 * The confirmation before an app is ended: names the app, says how many processes end and that
 * unsaved work is lost. Windows Explorer gets one more warning, because ending it takes the
 * taskbar and the desktop with it. Cancel has focus when it opens.
 */
export function EndDialog({ group, ending, onCancel, onConfirm }: EndDialogProps) {
  const count = group?.endCount ?? 0;
  return (
    <Dialog
      open={group !== null}
      onClose={() => {
        if (!ending) onCancel();
      }}
      title={group ? t('processes.dialog.title', { name: group.name }) : ''}
      description={
        group
          ? t('processes.dialog.description', { processes: processesText(count), name: group.name })
          : undefined
      }
      footer={
        <>
          <Button onClick={onCancel} disabled={ending}>
            {t('common.cancel')}
          </Button>
          <Button variant="primary" loading={ending} onClick={onConfirm}>
            {ending ? t('processes.dialog.ending') : tn('processes.dialog.end', count)}
          </Button>
        </>
      }
    >
      {group?.endability === 'explorer' ? (
        <div className="flex items-start gap-3 rounded-md bg-caution p-3 text-body text-caution-fg">
          <TriangleAlert aria-hidden size={18} className="mt-0.5 shrink-0" />
          <p>{t('processes.dialog.explorer')}</p>
        </div>
      ) : null}
    </Dialog>
  );
}
