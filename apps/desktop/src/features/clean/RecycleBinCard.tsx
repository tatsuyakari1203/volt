import { Loader2, Trash2, TriangleAlert } from 'lucide-react';
import { useCallback, useEffect, useRef, useState } from 'react';
import { emptyRecycleBin, getRecycleBinStatus, type RecycleBinView } from '../../shared/ipc';
import { Button, Card, Dialog, useToast } from '../../shared/ui';
import { countOf, formatSize } from './format';
import { describeError } from './messages';

type Load =
  | { status: 'loading' }
  | { status: 'error'; message: string }
  | { status: 'ready'; bin: RecycleBinView };

function holdsText(bin: RecycleBinView): string {
  return bin.itemCount === 0
    ? 'The Recycle Bin is empty.'
    : `${countOf(bin.itemCount, 'item')}, ${formatSize(bin.sizeBytes)}. These still use disk space.`;
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
          ? `The Recycle Bin was emptied. ${countOf(result.itemCount, 'item')} (${formatSize(result.sizeBytes)}) were permanently deleted.`
          : 'The Recycle Bin was already empty.',
      });
    } catch (error) {
      if (alive.current) {
        setNotice(
          `The Recycle Bin could not be emptied. ${describeError(error)} Some items may already have been deleted.`,
        );
      }
    } finally {
      if (alive.current) setEmptying(false);
      read();
    }
  };

  const bin = load.status === 'ready' ? load.bin : null;
  return (
    <Card as="section" aria-label="Recycle Bin" className="flex flex-col gap-3">
      <div className="flex items-center gap-3">
        <span
          aria-hidden
          className="grid size-9 shrink-0 place-items-center rounded-md bg-accent-soft text-accent-text"
        >
          <Trash2 size={18} />
        </span>
        <div className="min-w-0 flex-1">
          <h2 className="font-semibold">Recycle Bin</h2>
          {load.status === 'loading' ? (
            <p role="status" className="flex items-center gap-2 text-caption text-muted">
              <Loader2 aria-hidden size={12} className="animate-spin motion-reduce:animate-none" />
              Reading the Recycle Bin...
            </p>
          ) : null}
          {bin ? <p className="text-caption text-muted">{holdsText(bin)}</p> : null}
          {load.status === 'error' ? (
            <p className="text-caption text-muted">Could not read it. {load.message}</p>
          ) : null}
        </div>
        {load.status === 'error' ? (
          <Button size="sm" onClick={() => read()}>
            Try again
          </Button>
        ) : (
          <Button
            size="sm"
            loading={emptying}
            disabled={!bin || bin.itemCount === 0}
            onClick={() => setConfirming(true)}
          >
            Empty Recycle Bin
          </Button>
        )}
      </div>
      <p className="text-caption text-muted">
        Cleaning moves files here and does not free space. Emptying the Recycle Bin deletes
        everything in it for good, including what was there before Volt.
      </p>
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
        title="Empty the Recycle Bin for good?"
        description={
          bin
            ? `This permanently deletes ${countOf(bin.itemCount, 'item')} (${formatSize(bin.sizeBytes)}), including anything that was already there before Volt. This cannot be undone, and Volt cannot bring the files back.`
            : undefined
        }
        footer={
          <>
            <Button onClick={() => setConfirming(false)}>Cancel</Button>
            <Button variant="primary" onClick={() => void empty()}>
              Delete permanently
            </Button>
          </>
        }
      />
    </Card>
  );
}
