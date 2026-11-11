import { TriangleAlert } from 'lucide-react';
import { useEffect, useState } from 'react';
import { t, tn } from '../../shared/i18n';
import { listTweakDrift } from '../../shared/ipc';
import { Button } from '../../shared/ui';

/**
 * Tells the user, when Home opens, that tweaks Volt turned on have been changed since (a Windows
 * update resets many of them). The check only reads the system. It is advisory: when it fails or
 * finds nothing, nothing is shown. The Tweaks page lists them and offers to turn them on again.
 */
export function DriftNotice({ onReview }: { onReview: () => void }) {
  const [count, setCount] = useState(0);

  useEffect(() => {
    let alive = true;
    listTweakDrift()
      .then((found) => {
        if (alive) setCount(found.length);
      })
      .catch(() => {
        // Home does not depend on this check.
      });
    return () => {
      alive = false;
    };
  }, []);

  if (count === 0) return null;
  return (
    <section
      aria-label={t('home.drift.label')}
      className="flex items-start gap-3 rounded-md bg-caution p-3 text-body text-caution-fg"
    >
      <TriangleAlert aria-hidden size={18} className="mt-0.5 shrink-0" />
      <div className="min-w-0 flex-1">
        <p className="font-semibold">{tn('home.drift.title', count)}</p>
        <p>{t('home.drift.hint')}</p>
      </div>
      <Button size="sm" onClick={onReview}>
        {t('home.drift.review')}
      </Button>
    </section>
  );
}
