import { t, tn } from '../../shared/i18n';
import type { AppView } from '../../shared/ipc';
import { Button, Dialog } from '../../shared/ui';

export interface UninstallDialogProps {
  /** The apps that would be removed, or `null` while the dialog is closed. */
  apps: readonly AppView[] | null;
  onCancel: () => void;
  onConfirm: () => void;
}

/**
 * The confirmation before the queue starts: names every app, says that each one's own uninstaller
 * runs in turn and that Volt cannot undo it.
 */
export function UninstallDialog({ apps, onCancel, onConfirm }: UninstallDialogProps) {
  return (
    <Dialog
      open={apps !== null}
      onClose={onCancel}
      title={t('apps.confirm.title')}
      description={t('apps.confirm.description')}
      footer={
        <>
          <Button onClick={onCancel}>{t('common.cancel')}</Button>
          <Button variant="primary" onClick={onConfirm}>
            {tn('apps.uninstall', apps?.length ?? 0)}
          </Button>
        </>
      }
    >
      <ul
        aria-label={t('apps.confirm.list.label')}
        className="max-h-44 list-disc overflow-y-auto pl-5 text-body"
      >
        {apps?.map((app) => (
          <li key={app.id} className="wrap-anywhere">
            {app.name}
          </li>
        ))}
      </ul>
      <p className="mt-3 text-caption text-muted">{t('apps.confirm.note')}</p>
    </Dialog>
  );
}
