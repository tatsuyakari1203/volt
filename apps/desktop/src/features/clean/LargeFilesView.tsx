import { FolderOpen, FolderSearch, Loader2, TriangleAlert } from 'lucide-react';
import { useEffect, useMemo, useRef, useState } from 'react';
import { formatNumber, say, t, tn } from '../../shared/i18n';
import {
  cancelLargeFilesScan,
  IpcCommandError,
  type LargeFilesResult,
  type LargeFilesTrashResult,
  onLargeFilesProgress,
  pickScanFolder,
  revealLargeFile,
  scanLargeFiles,
  trashLargeFiles,
} from '../../shared/ipc';
import { formatSize } from '../../shared/text/size';
import { Button, Card, Dialog, ListGroup } from '../../shared/ui';
import {
  AGE_FILTERS,
  type AgeFilter,
  formatDate,
  GROUP_FILTERS,
  type GroupFilter,
  scanFilter,
  selectedFiles,
  selectionLabel,
  splitPath,
  visibleFiles,
} from './largeFiles';
import { describeError } from './messages';

type Phase = 'idle' | 'scanning' | 'moving';

const SELECT_CLASS =
  'h-8 rounded-md border border-line bg-surface px-2 text-body text-fg focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-focus';

function Notice({ message, onDismiss }: { message: string; onDismiss: () => void }) {
  return (
    <div
      role="alert"
      className="flex items-start gap-3 rounded-md bg-caution p-3 text-body text-caution-fg"
    >
      <TriangleAlert aria-hidden size={18} className="mt-0.5 shrink-0" />
      <p className="min-w-0 flex-1">{message}</p>
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

/**
 * The Large files view of the Clean screen: choose a drive or folder, scan it (read-only, with
 * progress and cancel), filter the biggest files by type and age, open a file's folder, and move
 * the ticked files to the Recycle Bin after a confirmation that states the count and total size.
 * Nothing is ticked by default and nothing is ever deleted for good. The scan and every rule
 * (what is left out, what may be moved) live in the Rust core.
 */
export function LargeFilesView({ onMoved }: { onMoved?: () => void }) {
  const [phase, setPhase] = useState<Phase>('idle');
  const [root, setRoot] = useState<string | null>(null);
  const [scan, setScan] = useState<LargeFilesResult | null>(null);
  const [filesSeen, setFilesSeen] = useState<number | null>(null);
  const [cancelling, setCancelling] = useState(false);
  const [group, setGroup] = useState<GroupFilter>('all');
  const [age, setAge] = useState<AgeFilter>(0);
  const [ticked, setTicked] = useState<ReadonlySet<number>>(new Set());
  const [confirming, setConfirming] = useState(false);
  const [result, setResult] = useState<LargeFilesTrashResult | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const alive = useRef(true);

  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  const files = scan?.files;
  const visible = useMemo(
    () => (scan && files ? visibleFiles(files, group, age, scan.scannedAtMs) : []),
    [scan, files, group, age],
  );
  const selected = useMemo(() => selectedFiles(visible, ticked), [visible, ticked]);
  const busy = phase !== 'idle';

  const choose = async () => {
    try {
      const chosen = await pickScanFolder();
      if (chosen && alive.current) setRoot(chosen);
    } catch (error) {
      if (alive.current) setNotice(t('large.scan.failed', { error: describeError(error) }));
    }
  };

  const startScan = async (nextGroup: GroupFilter = group, nextAge: AgeFilter = age) => {
    setPhase('scanning');
    setFilesSeen(null);
    setCancelling(false);
    setNotice(null);
    setResult(null);
    let unlisten: (() => void) | undefined;
    try {
      unlisten = await onLargeFilesProgress((next) => {
        if (alive.current) setFilesSeen(next.filesSeen);
      });
    } catch {
      // No progress without the event bridge; the scan itself still works.
    }
    try {
      // The filter goes to the scan itself, so it finds files beyond the 200 biggest.
      const found = await scanLargeFiles(root, scanFilter(nextGroup, nextAge));
      if (!alive.current) return;
      if (!found.cancelled) {
        setScan(found);
        setTicked(new Set());
      }
    } catch (error) {
      if (alive.current) setNotice(t('large.scan.failed', { error: describeError(error) }));
    } finally {
      unlisten?.();
      if (alive.current) setPhase('idle');
    }
  };

  const cancel = () => {
    setCancelling(true);
    cancelLargeFilesScan().catch(() => {});
  };

  const toggle = (index: number, on: boolean) => {
    setTicked((current) => {
      const next = new Set(current);
      if (on) next.add(index);
      else next.delete(index);
      return next;
    });
  };

  const open = (index: number) => {
    if (!scan) return;
    revealLargeFile(scan.scanId, index).catch((error: unknown) => {
      if (alive.current) setNotice(t('large.row.openFailed', { error: describeError(error) }));
    });
  };

  const move = async () => {
    if (!scan) return;
    setConfirming(false);
    setPhase('moving');
    setNotice(null);
    try {
      const done = await trashLargeFiles(
        scan.scanId,
        selected.map((file) => file.index),
      );
      if (!alive.current) return;
      setResult(done);
      setScan({ ...scan, files: done.files });
      setTicked(new Set());
      onMoved?.();
    } catch (error) {
      if (!alive.current) return;
      setNotice(t('large.move.failed', { error: describeError(error) }));
      if (error instanceof IpcCommandError && error.error.kind === 'notFound') setScan(null);
    } finally {
      if (alive.current) setPhase('idle');
    }
  };

  const scanning = phase === 'scanning';
  const rootText = root ?? t('large.where.system');

  return (
    <div className="flex flex-col gap-3">
      {notice ? <Notice message={notice} onDismiss={() => setNotice(null)} /> : null}

      <Card className="flex flex-col gap-3">
        <div className="flex flex-col gap-1">
          <p className="text-caption font-semibold text-muted">{t('large.where.label')}</p>
          <p className="wrap-anywhere font-mono text-body">
            {scan && !root ? scan.root : rootText}
          </p>
        </div>
        <div className="flex flex-wrap gap-2">
          <Button disabled={busy} onClick={() => void choose()}>
            {t('large.choose')}
          </Button>
          <Button variant="primary" disabled={busy || scanning} onClick={() => void startScan()}>
            {scan ? t('large.rescan') : t('large.scan')}
          </Button>
        </div>
        <p className="text-caption text-muted">{t('large.readonly')}</p>
      </Card>

      {scanning ? (
        <Card role="status" className="flex items-center gap-2">
          <Loader2
            aria-hidden
            size={18}
            className="shrink-0 animate-spin motion-reduce:animate-none"
          />
          <p className="min-w-0 flex-1">
            {cancelling
              ? t('large.stopping')
              : filesSeen === null
                ? t('large.starting')
                : t('large.scanning', { files: tn('count.file', filesSeen) })}
          </p>
          <Button size="sm" disabled={cancelling} onClick={cancel}>
            {t('common.cancel')}
          </Button>
        </Card>
      ) : null}

      {result && !scanning ? (
        <Card as="section" aria-label={t('large.result.label')} className="flex flex-col gap-2">
          <h2 className="font-semibold">{t('large.result.title')}</h2>
          <p>
            {t('large.result.moved', {
              files: tn('count.file', result.movedFiles),
              size: formatSize(result.movedBytes),
            })}
            {result.skippedTotal > 0
              ? ` ${t('large.result.left', { files: tn('count.file', result.skippedTotal) })}`
              : ''}
          </p>
          {result.skipped.length > 0 ? (
            <ul className="text-caption text-muted">
              {result.skipped.map((skip) => (
                <li key={skip.reason}>
                  {t('clean.result.group', {
                    items: tn('count.file', skip.count),
                    reason: say(skip.reasonPhrase, skip.reason),
                  })}
                </li>
              ))}
            </ul>
          ) : null}
          {result.historyIncomplete ? (
            <p className="text-caption text-muted">{t('large.result.historyIncomplete')}</p>
          ) : null}
        </Card>
      ) : null}

      {!scan && !scanning ? (
        <Card className="flex flex-col items-center gap-2 py-8 text-center">
          <FolderSearch aria-hidden size={24} className="text-muted" />
          <p className="font-semibold">{t('large.empty.title')}</p>
          <p className="max-w-xs text-body text-muted">{t('large.empty.description')}</p>
        </Card>
      ) : null}

      {scan && !scanning ? (
        <>
          <p className="text-caption text-muted">
            {t('large.summary', {
              count: scan.files.length,
              root: scan.root,
              checked: formatNumber(scan.filesSeen),
            })}
            {scan.unreadable > 0 || scan.links > 0
              ? ` ${t('large.skippedNote', {
                  unreadable: formatNumber(scan.unreadable),
                  links: formatNumber(scan.links),
                })}`
              : ''}
          </p>

          <div className="flex flex-wrap gap-3">
            <label className="flex flex-col gap-1 text-caption font-semibold text-muted">
              {t('large.filter.type')}
              <select
                className={SELECT_CLASS}
                value={group}
                disabled={busy}
                onChange={(event) => {
                  const next = event.target.value as GroupFilter;
                  setGroup(next);
                  void startScan(next, age);
                }}
              >
                {GROUP_FILTERS.map((value) => (
                  <option key={value} value={value}>
                    {t(`large.group.${value}`)}
                  </option>
                ))}
              </select>
            </label>
            <label className="flex flex-col gap-1 text-caption font-semibold text-muted">
              {t('large.filter.age')}
              <select
                className={SELECT_CLASS}
                value={age}
                disabled={busy}
                onChange={(event) => {
                  const next = Number(event.target.value) as AgeFilter;
                  setAge(next);
                  void startScan(group, next);
                }}
              >
                {AGE_FILTERS.map((value) => (
                  <option key={value} value={value}>
                    {value === 0
                      ? t('large.age.any')
                      : t('large.age.older', { days: tn('count.day', value) })}
                  </option>
                ))}
              </select>
            </label>
          </div>
          <p className="text-caption text-muted">{t('large.filter.hint')}</p>

          {visible.length === 0 ? (
            <Card className="text-center text-muted">
              {scan.files.length === 0 && group === 'all' && age === 0
                ? t('large.none')
                : t('large.noMatch')}
            </Card>
          ) : (
            <>
              <ListGroup aria-label={t('large.list.label')}>
                {visible.map((file) => {
                  const { name, folder } = splitPath(file.path);
                  return (
                    <li key={file.path} className="flex items-center gap-3 px-4 py-3">
                      <input
                        type="checkbox"
                        className="size-4 shrink-0 accent-(--color-accent)"
                        aria-label={t('large.row.select', { name })}
                        checked={ticked.has(file.index)}
                        disabled={busy}
                        onChange={(event) => toggle(file.index, event.target.checked)}
                      />
                      <div className="min-w-0 flex-1">
                        <p className="truncate font-semibold" title={file.path}>
                          {name}
                        </p>
                        <p className="truncate text-caption text-muted" title={folder}>
                          {folder}
                        </p>
                        <p className="text-caption text-muted">
                          {formatSize(file.sizeBytes)} ·{' '}
                          {file.modifiedMs === null
                            ? t('large.row.unknownDate')
                            : t('large.row.modified', { date: formatDate(file.modifiedMs) })}
                        </p>
                      </div>
                      <Button
                        size="sm"
                        variant="ghost"
                        aria-label={t('large.row.open', { name })}
                        title={t('large.row.open', { name })}
                        disabled={busy}
                        icon={<FolderOpen aria-hidden size={16} />}
                        onClick={() => open(file.index)}
                      />
                    </li>
                  );
                })}
              </ListGroup>

              <Card as="section" className="flex flex-col gap-3">
                <p>
                  {selected.length === 0
                    ? t('large.selection.none')
                    : t('large.selection.some', { selection: selectionLabel(selected) })}
                </p>
                <div>
                  <Button
                    variant="primary"
                    disabled={selected.length === 0 || busy}
                    loading={phase === 'moving'}
                    onClick={() => setConfirming(true)}
                  >
                    {t('large.move')}
                  </Button>
                </div>
              </Card>
            </>
          )}
        </>
      ) : null}

      <Dialog
        open={confirming}
        onClose={() => setConfirming(false)}
        title={t('large.confirm.title')}
        description={t('large.confirm.description', { selection: selectionLabel(selected) })}
        footer={
          <>
            <Button onClick={() => setConfirming(false)}>{t('common.cancel')}</Button>
            <Button variant="primary" onClick={() => void move()}>
              {t('large.move')}
            </Button>
          </>
        }
      />
    </div>
  );
}
