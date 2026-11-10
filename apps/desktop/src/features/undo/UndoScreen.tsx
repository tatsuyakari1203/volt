import { ClipboardCopy, Download, History, Loader2, TriangleAlert } from 'lucide-react';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  type ActivityItem,
  exportActivity,
  exportActivityText,
  historyStatus,
  IpcUnavailableError,
  listActivity,
  previewUndoEntry,
  type UndoPreview,
  type UndoResult,
  undoBatch,
  undoEntry,
} from '../../shared/ipc';
import { Button, Card, cx, PageHeader } from '../../shared/ui';
import { type BatchTarget, BatchUndoDialog } from './BatchUndoDialog';
import { type EntryTarget, EntryUndoDialog } from './EntryUndoDialog';
import { HistoryList } from './HistoryList';
import { groupHistory } from './history';
import {
  describeCopied,
  describeCopyFailure,
  describeError,
  describeSaved,
  describeSaveFailure,
  describeUndo,
  describeUndoFailure,
  type Notice,
} from './messages';

/** Records read per page. */
const PAGE_SIZE = 50;
/** The most records the shell returns in one page. */
const MAX_PAGE = 200;

type Load =
  | { status: 'loading' }
  | { status: 'error'; message: string }
  | { status: 'ready'; items: ActivityItem[]; nextCursor: string | null };

function describeLoadError(error: unknown): string {
  return error instanceof IpcUnavailableError
    ? 'The history is only available inside the Volt app.'
    : describeError(error);
}

/**
 * The Undo screen: what Volt did to this PC, newest first, from the activity log. Every row shows
 * the time, who started it, what changed in words, the previous value and whether it can be
 * undone. A single change can be undone from its row and a whole batch from its group, after
 * confirming what will be restored. The log can be saved as a text file or copied. All rules
 * live in the Rust core: this renders what the commands report, and reads the list again after
 * every undo so the rows are never older than the journal.
 */
export function UndoScreen() {
  const [load, setLoad] = useState<Load>({ status: 'loading' });
  const [more, setMore] = useState<'idle' | 'loading' | 'error'>('idle');
  const [busy, setBusy] = useState<string | null>(null);
  const [exporting, setExporting] = useState<'save' | 'copy' | null>(null);
  const [notice, setNotice] = useState<Notice | null>(null);
  const [confirming, setConfirming] = useState<BatchTarget | null>(null);
  const [confirmingEntry, setConfirmingEntry] = useState<EntryTarget | null>(null);
  const [incomplete, setIncomplete] = useState(false);
  const alive = useRef(true);
  const lastShown = useRef<string | null>(null);

  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  const items = load.status === 'ready' ? load.items : [];
  lastShown.current = items.length > 0 ? (items[items.length - 1]?.id ?? null) : null;
  const entries = useMemo(() => groupHistory(items), [items]);

  /** Asks the shell whether any activity record failed to be written; never blocks the list. */
  const checkHistory = useCallback(() => {
    historyStatus()
      .then((status) => {
        if (alive.current) setIncomplete(status.incomplete);
      })
      .catch(() => {});
  }, []);

  const read = useCallback(() => {
    checkHistory();
    setLoad({ status: 'loading' });
    listActivity(PAGE_SIZE)
      .then((page) => {
        if (alive.current) {
          setLoad({ status: 'ready', items: page.items, nextCursor: page.nextCursor });
        }
      })
      .catch((error: unknown) => {
        if (alive.current) setLoad({ status: 'error', message: describeLoadError(error) });
      });
  }, [checkHistory]);

  useEffect(read, [read]);

  /**
   * Reads the list again without hiding it, keeping every record that was shown, however many
   * pages that took, so a row's state is never older than the journal after an undo. New records
   * (the undo itself) arrive at the top, so it reads down to the oldest record that was loaded.
   */
  const refresh = useCallback(async () => {
    checkHistory();
    const oldest = lastShown.current;
    try {
      const loaded: ActivityItem[] = [];
      let cursor: string | undefined;
      let next: string | null = null;
      for (;;) {
        const page = await listActivity(MAX_PAGE, cursor);
        loaded.push(...page.items);
        next = page.nextCursor;
        const reached = oldest === null || page.items.some((row) => row.id === oldest);
        if (next === null || (reached && loaded.length >= PAGE_SIZE)) break;
        cursor = next;
      }
      if (alive.current) setLoad({ status: 'ready', items: loaded, nextCursor: next });
    } catch (error) {
      if (alive.current) {
        setNotice({
          tone: 'problem',
          message: `The list could not be refreshed. ${describeError(error)}`,
          details: [],
        });
      }
    }
  }, [checkHistory]);

  const older = (cursor: string) => {
    setMore('loading');
    listActivity(PAGE_SIZE, cursor)
      .then((page) => {
        if (!alive.current) return;
        setLoad((current) =>
          current.status === 'ready'
            ? {
                status: 'ready',
                items: [...current.items, ...page.items],
                nextCursor: page.nextCursor,
              }
            : current,
        );
        setMore('idle');
      })
      .catch(() => {
        if (alive.current) setMore('error');
      });
  };

  /** Runs an undo, reports how it ended and reads the list again. */
  const runUndo = async (
    key: string,
    scope: 'change' | 'batch',
    run: () => Promise<UndoResult>,
  ) => {
    setBusy(key);
    setNotice(null);
    let outcome: Notice;
    try {
      outcome = describeUndo(await run(), scope);
    } catch (error) {
      outcome = describeUndoFailure(error);
    }
    // The journal may have changed even when the command failed, so the list is read again.
    await refresh();
    if (!alive.current) return;
    setNotice(outcome);
    setBusy(null);
  };

  const undoOne = async (item: ActivityItem) => {
    if (item.entry === null) return;
    const entry = item.entry;
    // The core says whether a later change to the same setting would be overwritten; if so, the
    // user confirms first.
    setBusy(`entry:${entry}`);
    setNotice(null);
    let preview: UndoPreview;
    try {
      preview = await previewUndoEntry(entry);
    } catch (error) {
      if (alive.current) {
        setBusy(null);
        setNotice({
          tone: 'problem',
          message: `Could not check what this undo would change. ${describeError(error)}`,
          details: [],
        });
      }
      return;
    }
    if (!alive.current) return;
    if (preview.overwrites.length > 0) {
      setBusy(null);
      setConfirmingEntry({ entry, preview });
      return;
    }
    await runUndo(`entry:${entry}`, 'change', () => undoEntry(entry));
  };

  const confirmEntry = (target: EntryTarget) => {
    setConfirmingEntry(null);
    void runUndo(`entry:${target.entry}`, 'change', () => undoEntry(target.entry));
  };

  const confirmBatch = (target: BatchTarget) => {
    setConfirming(null);
    void runUndo(`batch:${target.batch}`, 'batch', () => undoBatch(target.batch));
  };

  const save = async () => {
    setExporting('save');
    setNotice(null);
    try {
      const outcome = describeSaved(await exportActivity());
      if (alive.current && outcome) setNotice(outcome);
    } catch (error) {
      if (alive.current) setNotice(describeSaveFailure(error));
    } finally {
      if (alive.current) setExporting(null);
    }
  };

  const copy = async () => {
    setExporting('copy');
    setNotice(null);
    try {
      const exported = await exportActivityText();
      await navigator.clipboard.writeText(exported.text);
      if (alive.current) setNotice(describeCopied(exported.records));
    } catch (error) {
      if (alive.current) setNotice(describeCopyFailure(error));
    } finally {
      if (alive.current) setExporting(null);
    }
  };

  const nextCursor = load.status === 'ready' ? load.nextCursor : null;
  const hasRecords = items.length > 0;

  return (
    <>
      <PageHeader
        title="Undo"
        description="Every change Volt made, newest first. Undo one change, or all the changes made together."
      />
      <div className="flex flex-col gap-3">
        {incomplete ? <IncompleteNotice /> : null}
        {notice ? <NoticeBar notice={notice} onDismiss={() => setNotice(null)} /> : null}

        {hasRecords ? (
          <div className="flex items-center justify-between gap-2">
            <p className="text-caption text-muted">
              {items.length === 1 ? '1 record' : `${items.length} records`}
              {nextCursor ? ' shown' : ''}
            </p>
            <div className="flex gap-2">
              <Button
                size="sm"
                icon={<Download aria-hidden size={16} />}
                loading={exporting === 'save'}
                disabled={exporting === 'copy'}
                onClick={() => void save()}
              >
                Export
              </Button>
              <Button
                size="sm"
                icon={<ClipboardCopy aria-hidden size={16} />}
                loading={exporting === 'copy'}
                disabled={exporting === 'save'}
                onClick={() => void copy()}
              >
                Copy
              </Button>
            </div>
          </div>
        ) : null}

        {load.status === 'loading' ? (
          <Card role="status" className="flex items-center justify-center gap-2 py-10 text-muted">
            <Loader2 aria-hidden size={18} className="animate-spin motion-reduce:animate-none" />
            <p>Reading the history...</p>
          </Card>
        ) : null}

        {load.status === 'error' ? (
          <Card role="alert" className="flex flex-col items-center gap-3 py-10 text-center">
            <TriangleAlert aria-hidden size={24} className="text-muted" />
            <p className="font-semibold">The history could not be loaded</p>
            <p className="max-w-xs text-body text-muted">{load.message}</p>
            <Button onClick={read}>Try again</Button>
          </Card>
        ) : null}

        {load.status === 'ready' && !hasRecords ? (
          <Card className="flex flex-col items-center gap-2 py-10 text-center">
            <History aria-hidden size={24} className="text-muted" />
            <p className="font-semibold">Nothing yet</p>
            <p className="max-w-xs text-body text-muted">
              When Volt changes something, it is listed here, with the value it replaced, so you can
              undo it.
            </p>
          </Card>
        ) : null}

        {hasRecords ? (
          <>
            <HistoryList
              entries={entries}
              busy={busy}
              onUndoEntry={(item) => void undoOne(item)}
              onUndoBatch={(batch, title) => setConfirming({ batch, title })}
            />
            {nextCursor ? (
              <div className="flex flex-col items-center gap-2">
                {more === 'error' ? (
                  <p role="alert" className="text-body text-muted">
                    Older entries could not be loaded.
                  </p>
                ) : null}
                <Button loading={more === 'loading'} onClick={() => older(nextCursor)}>
                  Show older
                </Button>
              </div>
            ) : null}
          </>
        ) : null}
      </div>
      <EntryUndoDialog
        target={confirmingEntry}
        onClose={() => setConfirmingEntry(null)}
        onConfirm={confirmEntry}
      />
      <BatchUndoDialog
        target={confirming}
        onClose={() => setConfirming(null)}
        onConfirm={confirmBatch}
      />
    </>
  );
}

const toneClasses: Record<Notice['tone'], string> = {
  done: 'bg-safe text-safe-fg',
  info: 'border border-line bg-surface-sunken text-fg',
  problem: 'bg-caution text-caution-fg',
};

/** A dismissible message under the header: how the last action ended, with what failed. */
function NoticeBar({ notice, onDismiss }: { notice: Notice; onDismiss: () => void }) {
  return (
    <div
      role={notice.tone === 'problem' ? 'alert' : 'status'}
      className={cx('flex items-start gap-3 rounded-md p-3 text-body', toneClasses[notice.tone])}
    >
      <div className="min-w-0 flex-1">
        <p>{notice.message}</p>
        {notice.details.length > 0 ? (
          <ul className="mt-1 list-disc space-y-0.5 pl-5">
            {notice.details.map((detail) => (
              <li key={detail} className="break-words">
                {detail}
              </li>
            ))}
          </ul>
        ) : null}
      </div>
      <button
        type="button"
        className="shrink-0 rounded-sm px-2 py-1 font-semibold underline hover:no-underline"
        onClick={onDismiss}
      >
        Dismiss
      </button>
    </div>
  );
}

/** Shown while the history is known to be missing records: the changes themselves are safe. */
function IncompleteNotice() {
  return (
    <div
      role="status"
      className="flex items-start gap-2 rounded-md border border-line bg-surface-sunken p-3 text-body"
    >
      <TriangleAlert aria-hidden size={18} className="mt-0.5 shrink-0 text-muted" />
      <p>
        This history is incomplete: some changes could not be written to it since Volt started.
        Those changes were still made, but they may be missing from this list.
      </p>
    </div>
  );
}
