import { Layers } from 'lucide-react';
import { useCallback, useEffect, useRef, useState } from 'react';
import { localeTag, t, tx } from '../../shared/i18n';
import {
  listProfiles,
  type ProfilesView,
  type SwitchResultView,
  undoProfileSwitch,
} from '../../shared/ipc';
import { Button, Card, Dialog } from '../../shared/ui';
import { describeError } from './messages';

export interface ProfileSwitchCardProps {
  /** Called after the switch was undone (whole or part), so the history is read again. */
  onUndone: () => void;
}

function names(items: readonly { id: string; title: string }[]): string {
  return items.length === 0
    ? t('undo.profile.nothing')
    : items.map((item) => tx(`tweak.${item.id}.title`, item.title)).join(', ');
}

/**
 * The last profile switch as one entry of the Undo page: which profile, what it turned on and
 * off, and one button that puts all of it back after a confirmation. A switch that stopped
 * part-way is listed too, so what it did can be undone. Nothing is shown without a switch.
 */
export function ProfileSwitchCard({ onUndone }: ProfileSwitchCardProps) {
  const [view, setView] = useState<ProfilesView | null>(null);
  const [asking, setAsking] = useState(false);
  const [busy, setBusy] = useState(false);
  const [outcome, setOutcome] = useState<{ result: SwitchResultView } | { error: string } | null>(
    null,
  );
  const cancelRef = useRef<HTMLButtonElement>(null);

  const load = useCallback(() => {
    listProfiles()
      .then(setView)
      .catch(() => {
        // Outside the app, or the profiles are unavailable: the card stays hidden.
      });
  }, []);
  useEffect(load, [load]);

  async function undo() {
    setBusy(true);
    try {
      setOutcome({ result: await undoProfileSwitch() });
    } catch (error) {
      setOutcome({ error: describeError(error) });
    }
    setBusy(false);
    setAsking(false);
    load();
    onUndone();
  }

  const record = view?.lastSwitch ?? null;
  const target = view?.profiles.find((profile) => profile.id === record?.to)?.name ?? null;
  const origin = view?.profiles.find((profile) => profile.id === record?.from)?.name ?? null;
  if (!view || (!record && !outcome)) return null;

  return (
    <>
      <Card as="section" aria-label={t('undo.profile.title')} className="flex gap-3">
        <Layers aria-hidden size={18} className="mt-0.5 shrink-0 text-muted" />
        <div className="min-w-0 flex-1">
          <h2 className="font-semibold">{t('undo.profile.title')}</h2>
          {record ? (
            <>
              <p className="text-body text-muted">
                {t(record.complete ? 'undo.profile.line' : 'undo.profile.partial', {
                  when: new Date(record.at).toLocaleString(localeTag(), {
                    dateStyle: 'medium',
                    timeStyle: 'short',
                  }),
                  name: target ?? t('undo.profile.none'),
                })}
              </p>
              <p className="text-caption text-muted">
                {t('undo.profile.changes', {
                  on: names(record.applied),
                  off: names(record.reverted),
                })}
              </p>
              <div className="mt-2">
                <Button size="sm" onClick={() => setAsking(true)}>
                  {t('undo.profile.button')}
                </Button>
              </div>
            </>
          ) : null}
          {outcome ? (
            <p role="status" className="mt-1 text-caption text-muted">
              {'error' in outcome
                ? t('undo.profile.failed', { message: outcome.error })
                : t(outcome.result.complete ? 'undo.profile.done' : 'undo.profile.stopped')}
            </p>
          ) : null}
        </div>
      </Card>
      <Dialog
        open={asking && record !== null}
        onClose={() => (busy ? undefined : setAsking(false))}
        title={t('undo.profile.confirm.title')}
        description={t('undo.profile.confirm.description')}
        initialFocusRef={cancelRef}
        footer={
          <>
            <Button ref={cancelRef} disabled={busy} onClick={() => setAsking(false)}>
              {t('common.cancel')}
            </Button>
            <Button variant="primary" loading={busy} onClick={() => void undo()}>
              {t('undo.profile.confirm.button')}
            </Button>
          </>
        }
      >
        {record ? (
          <ul className="space-y-2 text-body">
            <li>{t('undo.profile.confirm.off', { names: names(record.applied) })}</li>
            <li>{t('undo.profile.confirm.on', { names: names(record.reverted) })}</li>
            {record.automationChanged ? <li>{t('undo.profile.confirm.automation')}</li> : null}
            <li className="text-muted">
              {t('undo.profile.confirm.active', { name: origin ?? t('undo.profile.none') })}
            </li>
          </ul>
        ) : null}
      </Dialog>
    </>
  );
}
