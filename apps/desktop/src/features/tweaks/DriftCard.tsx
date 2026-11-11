import { TriangleAlert } from 'lucide-react';
import { useRef, useState } from 'react';
import { t, tn } from '../../shared/i18n';
import type { DriftView, TweakInfo } from '../../shared/ipc';
import { Button, Card, Dialog, ListGroup, ListRow } from '../../shared/ui';
import { tweakTitle } from './messages';
import { type Busy, StepList } from './TweakRow';

export interface DriftCardProps {
  /** What the read-only check found. */
  drift: readonly DriftView[];
  /** Every tweak, to name the drifted ones. A drifted id that is not here is not shown. */
  tweaks: readonly TweakInfo[];
  busy: Readonly<Record<string, Busy>>;
  /** Turn the tweak on again: a normal apply, so it asks first when the tweak is not Safe. */
  onReapply: (tweak: TweakInfo) => void;
  /** Stop treating the tweak as turned on; the settings stay as they are. */
  onKeep: (tweak: TweakInfo) => void;
}

/**
 * Tweaks that Volt turned on and that have been changed since (a Windows update resets many of
 * them). Each can be turned on again or its new state kept. Keeping asks first, because the undo
 * record of the tweak can no longer be used afterwards.
 */
export function DriftCard({ drift, tweaks, busy, onReapply, onKeep }: DriftCardProps) {
  const [keeping, setKeeping] = useState<TweakInfo | null>(null);
  const cancelRef = useRef<HTMLButtonElement>(null);
  const rows = drift.flatMap((item) => {
    const tweak = tweaks.find((candidate) => candidate.id === item.id);
    return tweak ? [{ item, tweak }] : [];
  });
  if (rows.length === 0) return null;
  return (
    <>
      <Card as="section" aria-label={t('tweaks.drift.title')} padding="none">
        <div className="flex items-start gap-3 p-4 pb-3">
          <TriangleAlert aria-hidden size={18} className="mt-0.5 shrink-0 text-caution-fg" />
          <div className="min-w-0 flex-1">
            <h2 className="font-semibold">{t('tweaks.drift.title')}</h2>
            <p className="text-body text-muted">{tn('tweaks.drift.description', rows.length)}</p>
            <p className="text-caption text-muted">{t('tweaks.drift.legend')}</p>
          </div>
        </div>
        <ListGroup
          aria-label={t('tweaks.drift.title')}
          className="rounded-none border-x-0 border-b-0 shadow-none"
        >
          {rows.map(({ item, tweak }) => {
            const working = busy[tweak.id] !== undefined;
            const title = tweakTitle(tweak);
            return (
              <ListRow
                key={tweak.id}
                title={title}
                description={
                  <>
                    <span className="block font-semibold text-fg">
                      {t(
                        item.state.kind === 'partial'
                          ? 'tweaks.drift.partial'
                          : 'tweaks.drift.gone',
                      )}
                    </span>
                    <StepList
                      label={t('tweaks.drift.changes', { title })}
                      steps={item.steps.filter((step) => !step.atTarget)}
                    />
                    {tweak.needsAdmin ? (
                      <span className="mt-1 block">{t('tweaks.row.needsAdmin')}</span>
                    ) : null}
                    <span className="mt-2 flex flex-wrap gap-2">
                      <Button
                        size="sm"
                        variant="primary"
                        aria-label={t('tweaks.drift.reapply.label', { title })}
                        disabled={working || tweak.needsAdmin}
                        onClick={() => onReapply(tweak)}
                      >
                        {t('tweaks.drift.reapply')}
                      </Button>
                      <Button
                        size="sm"
                        aria-label={t('tweaks.drift.keep.label', { title })}
                        disabled={working}
                        onClick={() => setKeeping(tweak)}
                      >
                        {t('tweaks.drift.keep')}
                      </Button>
                    </span>
                  </>
                }
              />
            );
          })}
        </ListGroup>
      </Card>
      <Dialog
        open={keeping !== null}
        onClose={() => setKeeping(null)}
        title={t('tweaks.drift.keep.title')}
        description={
          keeping ? t('tweaks.drift.keep.description', { title: tweakTitle(keeping) }) : undefined
        }
        initialFocusRef={cancelRef}
        footer={
          <>
            <Button ref={cancelRef} onClick={() => setKeeping(null)}>
              {t('common.cancel')}
            </Button>
            <Button
              variant="primary"
              onClick={() => {
                const target = keeping;
                setKeeping(null);
                if (target) onKeep(target);
              }}
            >
              {t('tweaks.drift.keep.confirm')}
            </Button>
          </>
        }
      >
        <p className="text-body text-muted">{t('tweaks.drift.keep.undo')}</p>
      </Dialog>
    </>
  );
}
