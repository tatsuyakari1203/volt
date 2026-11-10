import { Loader2, Trash2, TriangleAlert } from 'lucide-react';
import { useCallback, useEffect, useRef, useState } from 'react';
import { t, tn } from '../../shared/i18n';
import { emptyRecycleBin, getRecycleBinStatus, type RecycleBinView } from '../../shared/ipc';
import { formatSize } from '../../shared/text/size';
import { Button, Card, Dialog, useToast } from '../../shared/ui';
import { describeError } from './messages';

type Load =
  | { status: 'loading' }
  | { status: 'error'; message: string }
  | { status: 'ready'; bin: RecycleBinView };

function holdsText(bin: RecycleBinView): string {
  return bin.itemCount === 0
    ? t('clean.bin.isEmpty')
    : t('clean.bin.holds', {
        items: tn('count.item', bin.itemCount),
        size: formatSize(bin.sizeBytes),
      });
}

/**
 * The Recycle Bin card: what it holds (a read-only query) and a button that empties it for good.
 * Cleaning only ever moves files here, so this is the one place space is actually freed, and it
 * cannot be undone: the dialog says so before anything is deleted. `refreshKey` changes after a
 * clean, so the numbers follow.
 */
export function RecycleBinCard({ refreshKey }: { refreshKey: number }) {
  const toast = useToast();
  const [load, setLoad] = useState<Load>({ status: 'loading' });
  const [confirming, setConfirming] = useState(false);
  const [emptying, setEmptying] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const alive = useRef(true);

  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  const read = useCallback(() => {
    getRecycleBinStatus()
      .then((bin) => {
        if (alive.current) setLoad({ status: 'ready', bin });
      })
      .catch((error: unknown) => {
        if (alive.current) setLoad({ status: 'error', message: describeError(error) });
      });
  }, []);

  // A clean elsewhere on the page changes `refreshKey`, which re-reads the bin.
  // biome-ignore lint/correctness/useExhaustiveDependencies: the key only triggers the read.
  useEffect(() => read(), [read, refreshKey]);

  const empty = async () => {
    setConfirming(false);
    setEmptying(true);
    setNotice(null);
    try {
      const result = await emptyRecycleBin();
      toast.show({
        message: result.emptied
          ? t('clean.bin.emptied', {
              items: tn('count.item', result.itemCount),
              size: formatSize(result.sizeBytes),
            })
          : t('clean.bin.alreadyEmpty'),
      });
    } catch (error) {
      if (alive.current) {
        setNotice(t('clean.bin.emptyFailed', { error: describeError(error) }));
      }
    } finally {
      if (alive.current) setEmptying(false);
      read();
    }
  };

  const bin = load.status === 'ready' ? load.bin : null;
  return (
    <Card as="section" aria-label={t('clean.bin.title')} className="flex flex-col gap-3">
      <div className="flex items-center gap-3">
        <span
          aria-hidden
          className="grid size-9 shrink-0 place-items-center rounded-md bg-accent-soft text-accent-text"
        >
          <Trash2 size={18} />
        </span>
        <div className="min-w-0 flex-1">
          <h2 className="font-semibold">{t('clean.bin.title')}</h2>
          {load.status === 'loading' ? (
            <p role="status" className="flex items-center gap-2 text-caption text-muted">
              <Loader2 aria-hidden size={12} className="animate-spin motion-reduce:animate-none" />
              {t('clean.bin.reading')}
            </p>
          ) : null}
          {bin ? <p className="text-caption text-muted">{holdsText(bin)}</p> : null}
          {load.status === 'error' ? (
            <p className="text-caption text-muted">
              {t('clean.bin.readFailed', { message: load.message })}
            </p>
          ) : null}
        </div>
        {load.status === 'error' ? (
          <Button size="sm" onClick={() => read()}>
            {t('common.tryAgain')}
          </Button>
        ) : (
          <Button
            size="sm"
            loading={emptying}
            disabled={!bin || bin.itemCount === 0}
            onClick={() => setConfirming(true)}
          >
            {t('clean.bin.emptyButton')}
          </Button>
        )}
      </div>
      <p className="text-caption text-muted">{t('clean.bin.note')}</p>
      {notice ? (
        <div
          role="alert"
          className="flex items-start gap-3 rounded-md bg-caution p-3 text-body text-caution-fg"
        >
          <TriangleAlert aria-hidden size={18} className="mt-0.5 shrink-0" />
          <p className="min-w-0 flex-1">{notice}</p>
        </div>
      ) : null}
      <Dialog
        open={confirming}
        onClose={() => setConfirming(false)}
        title={t('clean.bin.confirm.title')}
        description={
          bin
            ? t('clean.bin.confirm.description', {
                items: tn('count.item', bin.itemCount),
                size: formatSize(bin.sizeBytes),
              })
            : undefined
        }
        footer={
          <>
            <Button onClick={() => setConfirming(false)}>{t('common.cancel')}</Button>
            <Button variant="primary" onClick={() => void empty()}>
              {t('clean.bin.confirm.delete')}
            </Button>
          </>
        }
      />
    </Card>
  );
}
