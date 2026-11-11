import { ClipboardCopy, Download, History, Loader2, Trash2, TriangleAlert } from 'lucide-react';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { t, tn } from '../../shared/i18n';
import {
  type ActivityItem,
  clearHistory,
  exportActivity,
  exportActivityText,
  historyStatus,
  IpcUnavailableError,
  listActivity,
  previewUndoEntry,
  type UndoPreview,
  type UndoResult,
  undoAdministratorChange,
  undoBatch,
  undoEntry,
} from '../../shared/ipc';
import { Button, Card, cx, PageHeader } from '../../shared/ui';
import { type BatchTarget, BatchUndoDialog } from './BatchUndoDialog';
import { ClearHistoryDialog } from './ClearHistoryDialog';
import { type EntryTarget, EntryUndoDialog } from './EntryUndoDialog';
import { HistoryList } from './HistoryList';
import { groupHistory } from './history';
import {
  describeCleared,
  describeClearFailure,
  describeCopied,
  describeCopyFailure,
  describeError,
  describeSaved,
  describeSaveFailure,
  describeUndo,
  describeUndoFailure,
  type Notice,
} from './messages';
import { ProfileSwitchCard } from './ProfileSwitchCard';

/** Records read per page. */
const PAGE_SIZE = 50;
/** The most records the shell returns in one page. */
const MAX_PAGE = 200;

type Load =
  | { status: 'loading' }
  | { status: 'error'; message: string }
  | { status: 'ready'; items: ActivityItem[]; nextCursor: string | null };

function describeLoadError(error: unknown): string {
  return error instanceof IpcUnavailableError ? t('undo.loadOutsideApp') : describeError(error);
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
  const [clearing, setClearing] = useState<'closed' | 'asking' | 'running'>('closed');
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
          message: t('undo.refreshFailed', { error: describeError(error) }),
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

  /**
   * Runs an undo, reports how it ended and reads the list again. When the core refuses because a
   * later change appeared after the preview, `reopen` shows the warning again with the new
   * overwrites; the notice is shown only when it cannot (it returns `false`).
   */
  const runUndo = useCallback(
    async (
      key: string,
      scope: 'change' | 'batch',
      run: () => Promise<UndoResult>,
      reopen: () => Promise<boolean>,
    ) => {
      setBusy(key);
      setNotice(null);
      let outcome: Notice;
      let refused = false;
      try {
        const result = await run();
        refused = result.status === 'needsAcknowledgement';
        outcome = describeUndo(result, scope);
      } catch (error) {
        outcome = describeUndoFailure(error);
      }
      // The journal may have changed even when the command failed, so the list is read again.
      await refresh();
      if (!alive.current) return;
      if (refused && (await reopen())) {
        if (alive.current) setBusy(null);
        return;
      }
      if (!alive.current) return;
      setNotice(outcome);
      setBusy(null);
    },
    [refresh],
  );

  /** Reads a fresh preview of one change and opens the warning with it. `false` when it fails. */
  const reopenEntry = useCallback(async (entry: string): Promise<boolean> => {
    try {
      const preview = await previewUndoEntry(entry);
      if (!alive.current) return true;
      setConfirmingEntry({ entry, preview });
      return true;
    } catch {
      return false;
    }
  }, []);

  const undoOne = useCallback(
    async (item: ActivityItem) => {
      if (item.administrator) {
        // The helper confirms it itself and restores the earlier value from its own journal.
        const tweak = item.administratorTweak;
        if (tweak !== null) {
          await runUndo(
            `admin:${tweak}`,
            'change',
            () => undoAdministratorChange(tweak),
            async () => false,
          );
        }
        return;
      }
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
            message: t('undo.previewFailed', { error: describeError(error) }),
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
      await runUndo(
        `entry:${entry}`,
        'change',
        () => undoEntry(entry, []),
        () => reopenEntry(entry),
      );
    },
    [runUndo, reopenEntry],
  );

  const onUndoEntry = useCallback((item: ActivityItem) => void undoOne(item), [undoOne]);
  const onUndoBatch = useCallback(
    (batch: string, title: string) => setConfirming({ batch, title }),
    [],
  );

  const confirmEntry = (target: EntryTarget) => {
    setConfirmingEntry(null);
    const shown = target.preview.overwrites.map((found) => found.laterEntry);
    void runUndo(
      `entry:${target.entry}`,
      'change',
      () => undoEntry(target.entry, shown),
      () => reopenEntry(target.entry),
    );
  };

  const confirmBatch = (target: BatchTarget, shown: string[]) => {
    setConfirming(null);
    // The dialog reads its own preview when it opens, so opening it again is enough.
    void runUndo(
      `batch:${target.batch}`,
      'batch',
      () => undoBatch(target.batch, shown),
      async () => {
        setConfirming({ batch: target.batch, title: target.title });
        return true;
      },
    );
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

  const clear = async () => {
    setClearing('running');
    setNotice(null);
    let outcome: Notice;
    try {
      const cleared = await clearHistory();
      outcome = describeCleared(cleared.removed, cleared.kept);
    } catch (error) {
      outcome = describeClearFailure(error);
    }
    await refresh();
    if (!alive.current) return;
    setClearing('closed');
    setNotice(outcome);
  };

  const nextCursor = load.status === 'ready' ? load.nextCursor : null;
  const hasRecords = items.length > 0;

  return (
    <>
      <PageHeader title={t('nav.undo')} description={t('undo.description')} />
      <div className="flex flex-col gap-3">
        {incomplete ? <IncompleteNotice /> : null}
        {notice ? <NoticeBar notice={notice} onDismiss={() => setNotice(null)} /> : null}
        <ProfileSwitchCard onUndone={() => void refresh()} />

        {hasRecords ? (
          <div className="flex items-center justify-between gap-2">
            <p className="text-caption text-muted">
              {nextCursor
                ? t('undo.records.shown', { records: tn('count.record', items.length) })
                : tn('count.record', items.length)}
            </p>
            <div className="flex gap-2">
              <Button
                size="sm"
                icon={<Download aria-hidden size={16} />}
                loading={exporting === 'save'}
                disabled={exporting === 'copy'}
                onClick={() => void save()}
              >
                {t('undo.export')}
              </Button>
              <Button
                size="sm"
                icon={<ClipboardCopy aria-hidden size={16} />}
                loading={exporting === 'copy'}
                disabled={exporting === 'save'}
                onClick={() => void copy()}
              >
                {t('undo.copy')}
              </Button>
              <Button
                size="sm"
                icon={<Trash2 aria-hidden size={16} />}
                disabled={exporting !== null}
                onClick={() => setClearing('asking')}
              >
                {t('undo.clear')}
              </Button>
            </div>
          </div>
        ) : null}

        {load.status === 'loading' ? (
          <Card role="status" className="flex items-center justify-center gap-2 py-10 text-muted">
            <Loader2 aria-hidden size={18} className="animate-spin motion-reduce:animate-none" />
            <p>{t('undo.loading')}</p>
          </Card>
        ) : null}

        {load.status === 'error' ? (
          <Card role="alert" className="flex flex-col items-center gap-3 py-10 text-center">
            <TriangleAlert aria-hidden size={24} className="text-muted" />
            <p className="font-semibold">{t('undo.error')}</p>
            <p className="max-w-xs text-body text-muted">{load.message}</p>
            <Button onClick={read}>{t('common.tryAgain')}</Button>
          </Card>
        ) : null}

        {load.status === 'ready' && !hasRecords ? (
          <Card className="flex flex-col items-center gap-2 py-10 text-center">
            <History aria-hidden size={24} className="text-muted" />
            <p className="font-semibold">{t('undo.empty.title')}</p>
            <p className="max-w-xs text-body text-muted">{t('undo.empty.description')}</p>
          </Card>
        ) : null}

        {hasRecords ? (
          <>
            <HistoryList
              entries={entries}
              busy={busy}
              onUndoEntry={onUndoEntry}
              onUndoBatch={onUndoBatch}
            />
            {nextCursor ? (
              <div className="flex flex-col items-center gap-2">
                {more === 'error' ? (
                  <p role="alert" className="text-body text-muted">
                    {t('undo.olderFailed')}
                  </p>
                ) : null}
                <Button loading={more === 'loading'} onClick={() => older(nextCursor)}>
                  {t('undo.showOlder')}
                </Button>
              </div>
            ) : null}
          </>
        ) : null}
      </div>
      <ClearHistoryDialog
        open={clearing !== 'closed'}
        busy={clearing === 'running'}
        onClose={() => setClearing('closed')}
        onConfirm={() => void clear()}
      />
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
        {t('common.dismiss')}
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
      <p>{t('undo.incomplete')}</p>
    </div>
  );
}
