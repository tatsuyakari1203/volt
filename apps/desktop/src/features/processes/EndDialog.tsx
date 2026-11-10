import { TriangleAlert } from 'lucide-react';
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
      title={group ? `End ${group.name}?` : ''}
      description={
        group
          ? `${processesText(count)} of ${group.name} will end at once. Anything you have not saved in it is lost. This cannot be undone.`
          : undefined
      }
      footer={
        <>
          <Button onClick={onCancel} disabled={ending}>
            Cancel
          </Button>
          <Button variant="primary" loading={ending} onClick={onConfirm}>
            {ending ? 'Ending...' : `End ${count === 1 ? 'process' : `${count} processes`}`}
          </Button>
        </>
      }
    >
      {group?.endability === 'explorer' ? (
        <div className="flex items-start gap-3 rounded-md bg-caution p-3 text-body text-caution-fg">
          <TriangleAlert aria-hidden size={18} className="mt-0.5 shrink-0" />
          <p>
            This is Windows Explorer. Ending it closes the taskbar, the desktop and every File
            Explorer window until Windows starts it again, which it normally does within seconds.
          </p>
        </div>
      ) : null}
    </Dialog>
  );
}
