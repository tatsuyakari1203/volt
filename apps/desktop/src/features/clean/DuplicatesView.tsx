import { FolderOpen, FolderSearch, Loader2, TriangleAlert, X } from 'lucide-react';
import { type ReactNode, useEffect, useMemo, useRef, useState } from 'react';
import { formatNumber, say, t, tn } from '../../shared/i18n';
import {
  cancelTidyScan,
  IpcCommandError,
  onTidyProgress,
  pickScanFolder,
  revealTidyItem,
  scanTidy,
  type TidyItemKind,
  type TidyProgressEvent,
  type TidyResult,
  type TidyTrashResult,
  trashBrokenShortcuts,
  trashDuplicates,
  trashEmptyFolders,
} from '../../shared/ipc';
import { formatSize } from '../../shared/text/size';
import { Button, Card, Dialog } from '../../shared/ui';
import { formatDate, splitPath } from './largeFiles';
import { describeError } from './messages';
import {
  canMark,
  copiesLabel,
  keepOnly,
  type Marks,
  markedTotals,
  picksOf,
  pruneMarks,
  scanNotes,
  withMark,
} from './tidy';

type Phase = 'idle' | 'scanning' | 'moving';

/** What a confirmation is about to move. */
type Pending = 'duplicates' | 'folders' | 'shortcuts';

interface PreviewRow {
  path: string;
  sizeBytes: number | null;
}

/** How many rows of the preview are listed; the rest are counted. */
const PREVIEW_ROWS = 12;

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

function Section({ title, hint, children }: { title: string; hint: string; children: ReactNode }) {
  return (
    <section aria-label={title} className="flex flex-col gap-2">
      <h2 className="font-semibold">{title}</h2>
      <p className="text-caption text-muted">{hint}</p>
      {children}
    </section>
  );
}

function Checkbox({
  label,
  checked,
  disabled,
  title,
  onChange,
}: {
  label: string;
  checked: boolean;
  disabled?: boolean;
  title?: string;
  onChange: (on: boolean) => void;
}) {
  return (
    <input
      type="checkbox"
      className="size-4 shrink-0 accent-(--color-accent)"
      aria-label={label}
      title={title}
      checked={checked}
      disabled={disabled}
      onChange={(event) => onChange(event.target.checked)}
    />
  );
}

/**
 * The Duplicates view of the Clean screen: choose folders, scan them (read-only, with progress and
 * cancel) and get three separate lists: duplicate files, empty folders and shortcuts whose target
 * is missing. Nothing is marked by default. In a duplicate group the user picks the copy to keep
 * (the others get marked) and at least one copy always stays. Every move to the Recycle Bin
 * starts with a preview that states the count and sizes, and nothing is deleted for good. What
 * counts as a duplicate, an empty folder or a broken shortcut, and every re-check before a move,
 * live in the Rust core.
 */
export function DuplicatesView({ onMoved }: { onMoved?: () => void }) {
  const [phase, setPhase] = useState<Phase>('idle');
  const [roots, setRoots] = useState<string[]>([]);
  const [scan, setScan] = useState<TidyResult | null>(null);
  const [progress, setProgress] = useState<TidyProgressEvent | null>(null);
  const [cancelling, setCancelling] = useState(false);
  const [marks, setMarks] = useState<Marks>(new Map());
  const [folders, setFolders] = useState<ReadonlySet<number>>(new Set());
  const [shortcuts, setShortcuts] = useState<ReadonlySet<number>>(new Set());
  const [pending, setPending] = useState<Pending | null>(null);
  const [result, setResult] = useState<TidyTrashResult | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const alive = useRef(true);

  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  const lists = scan?.lists;
  const busy = phase !== 'idle';
  const scanning = phase === 'scanning';
  const totals = useMemo(
    () => markedTotals(lists?.duplicates ?? [], marks),
    [lists?.duplicates, marks],
  );
  const chosenFolders = useMemo(
    () => (lists?.emptyFolders ?? []).filter((folder) => folders.has(folder.index)),
    [lists?.emptyFolders, folders],
  );
  const chosenShortcuts = useMemo(
    () => (lists?.brokenShortcuts ?? []).filter((shortcut) => shortcuts.has(shortcut.index)),
    [lists?.brokenShortcuts, shortcuts],
  );

  const addFolder = async () => {
    try {
      const chosen = await pickScanFolder();
      if (chosen && alive.current)
        setRoots((current) => (current.includes(chosen) ? current : [...current, chosen]));
    } catch (error) {
      if (alive.current) setNotice(t('tidy.scan.failed', { error: describeError(error) }));
    }
  };

  const startScan = async () => {
    setPhase('scanning');
    setProgress(null);
    setCancelling(false);
    setNotice(null);
    setResult(null);
    let unlisten: (() => void) | undefined;
    try {
      unlisten = await onTidyProgress((next) => {
        if (alive.current) setProgress(next);
      });
    } catch {
      // No progress without the event bridge; the scan itself still works.
    }
    try {
      const found = await scanTidy(roots);
      if (!alive.current) return;
      if (!found.cancelled) {
        setScan(found);
        setMarks(new Map());
        setFolders(new Set());
        setShortcuts(new Set());
      }
    } catch (error) {
      if (alive.current) setNotice(t('tidy.scan.failed', { error: describeError(error) }));
    } finally {
      unlisten?.();
      if (alive.current) setPhase('idle');
    }
  };

  const cancel = () => {
    setCancelling(true);
    cancelTidyScan().catch(() => {});
  };

  const open = (kind: TidyItemKind, index: number, file?: number) => {
    if (!scan) return;
    revealTidyItem(scan.scanId, kind, index, file).catch((error: unknown) => {
      if (alive.current) setNotice(t('tidy.row.openFailed', { error: describeError(error) }));
    });
  };

  const toggle = (
    setter: (update: (current: ReadonlySet<number>) => ReadonlySet<number>) => void,
    index: number,
    on: boolean,
  ) =>
    setter((current) => {
      const next = new Set(current);
      if (on) next.add(index);
      else next.delete(index);
      return next;
    });

  const preview = (kind: Pending): PreviewRow[] => {
    if (kind === 'folders') return chosenFolders.map((f) => ({ path: f.path, sizeBytes: null }));
    if (kind === 'shortcuts')
      return chosenShortcuts.map((s) => ({ path: s.path, sizeBytes: s.sizeBytes }));
    return (lists?.duplicates ?? []).flatMap((group) =>
      [...(marks.get(group.index) ?? [])]
        .sort((a, b) => a - b)
        .flatMap((file) => {
          const copy = group.files[file];
          return copy ? [{ path: copy.path, sizeBytes: group.sizeBytes }] : [];
        }),
    );
  };

  const move = async () => {
    if (!scan || !pending) return;
    const kind = pending;
    setPending(null);
    setPhase('moving');
    setNotice(null);
    try {
      const done =
        kind === 'duplicates'
          ? await trashDuplicates(scan.scanId, picksOf(marks))
          : kind === 'folders'
            ? await trashEmptyFolders(
                scan.scanId,
                chosenFolders.map((folder) => folder.index),
              )
            : await trashBrokenShortcuts(
                scan.scanId,
                chosenShortcuts.map((shortcut) => shortcut.index),
              );
      if (!alive.current) return;
      setResult(done);
      setScan({ ...scan, lists: done.lists });
      setMarks((current) => pruneMarks(current, done.lists.duplicates));
      setFolders(new Set());
      setShortcuts(new Set());
      onMoved?.();
    } catch (error) {
      if (!alive.current) return;
      setNotice(t('tidy.move.failed', { error: describeError(error) }));
      if (error instanceof IpcCommandError && error.error.kind === 'notFound') setScan(null);
    } finally {
      if (alive.current) setPhase('idle');
    }
  };

  const progressText = (() => {
    if (cancelling) return t('tidy.stopping');
    if (!progress) return t('tidy.starting');
    if (progress.stage === 'shortcuts') return t('tidy.stage.shortcuts');
    if (progress.stage === 'comparing')
      return t('tidy.stage.comparing', {
        done: formatNumber(progress.compared),
        total: formatNumber(progress.toCompare),
        size: formatSize(progress.bytesHashed),
      });
    return t('tidy.stage.walking', { files: tn('count.file', progress.filesSeen) });
  })();

  const pendingRows = pending ? preview(pending) : [];
  const pendingBytes = pendingRows.reduce((sum, row) => sum + (row.sizeBytes ?? 0), 0);
  const pendingLabel =
    pending === 'duplicates'
      ? copiesLabel(pendingRows.length, pendingBytes)
      : pending === 'shortcuts'
        ? t('tidy.selection.sized', {
            items: tn('count.shortcut', pendingRows.length),
            size: formatSize(pendingBytes),
          })
        : tn('count.folder', pendingRows.length);

  return (
    <div className="flex flex-col gap-3">
      {notice ? <Notice message={notice} onDismiss={() => setNotice(null)} /> : null}

      <Card className="flex flex-col gap-3">
        <div className="flex flex-col gap-1">
          <p className="text-caption font-semibold text-muted">{t('tidy.where.label')}</p>
          {roots.length === 0 ? (
            <p className="text-body text-muted">{t('tidy.where.none')}</p>
          ) : (
            <ul className="flex flex-col gap-1">
              {roots.map((root) => (
                <li key={root} className="flex items-center gap-2">
                  <span className="min-w-0 flex-1 wrap-anywhere font-mono text-body">{root}</span>
                  <Button
                    size="sm"
                    variant="ghost"
                    disabled={busy}
                    aria-label={t('tidy.where.remove', { folder: root })}
                    title={t('tidy.where.remove', { folder: root })}
                    icon={<X aria-hidden size={16} />}
                    onClick={() => setRoots((current) => current.filter((r) => r !== root))}
                  />
                </li>
              ))}
            </ul>
          )}
        </div>
        <div className="flex flex-wrap gap-2">
          <Button disabled={busy} onClick={() => void addFolder()}>
            {t('tidy.add')}
          </Button>
          <Button
            variant="primary"
            disabled={busy || roots.length === 0}
            onClick={() => void startScan()}
          >
            {scan ? t('tidy.rescan') : t('tidy.scan')}
          </Button>
        </div>
        <p className="text-caption text-muted">{t('tidy.readonly')}</p>
      </Card>

      {scanning ? (
        <Card role="status" className="flex items-center gap-2">
          <Loader2
            aria-hidden
            size={18}
            className="shrink-0 animate-spin motion-reduce:animate-none"
          />
          <p className="min-w-0 flex-1">{progressText}</p>
          <Button size="sm" disabled={cancelling} onClick={cancel}>
            {t('common.cancel')}
          </Button>
        </Card>
      ) : null}

      {result && !scanning ? (
        <Card as="section" aria-label={t('tidy.result.label')} className="flex flex-col gap-2">
          <h2 className="font-semibold">{t('tidy.result.title')}</h2>
          <p>
            {result.movedBytes > 0
              ? t('tidy.result.moved', {
                  items: tn('count.item', result.moved),
                  size: formatSize(result.movedBytes),
                })
              : t('tidy.result.movedNoSize', { items: tn('count.item', result.moved) })}
            {result.skippedTotal > 0
              ? ` ${t('tidy.result.left', { items: tn('count.item', result.skippedTotal) })}`
              : ''}
          </p>
          {result.skipped.length > 0 ? (
            <ul className="text-caption text-muted">
              {result.skipped.map((skip) => (
                <li key={skip.reason}>
                  {t('clean.result.group', {
                    items: tn('count.item', skip.count),
                    reason: say(skip.reasonPhrase, skip.reason),
                  })}
                </li>
              ))}
            </ul>
          ) : null}
          {result.historyIncomplete ? (
            <p className="text-caption text-muted">{t('tidy.result.historyIncomplete')}</p>
          ) : null}
        </Card>
      ) : null}

      {!scan && !scanning ? (
        <Card className="flex flex-col items-center gap-2 py-8 text-center">
          <FolderSearch aria-hidden size={24} className="text-muted" />
          <p className="font-semibold">{t('tidy.empty.title')}</p>
          <p className="max-w-xs text-body text-muted">{t('tidy.empty.description')}</p>
        </Card>
      ) : null}

      {scan && lists && !scanning ? (
        <>
          <p className="text-caption text-muted">
            {t('tidy.summary', {
              checked: formatNumber(scan.filesSeen),
              roots: scan.roots.join(', '),
            })}{' '}
            {scanNotes(scan).join(' ')}
          </p>

          <Section
            title={t('tidy.dup.title')}
            hint={t('tidy.dup.hint', { size: formatSize(scan.minDuplicateBytes) })}
          >
            {lists.duplicates.length === 0 ? (
              <Card className="text-center text-muted">{t('tidy.dup.none')}</Card>
            ) : (
              <>
                {lists.duplicates.map((group) => {
                  const marked = marks.get(group.index);
                  const first = group.files[0]?.path ?? '';
                  return (
                    <Card
                      key={`${group.index}-${first}`}
                      as="section"
                      aria-label={t('tidy.dup.group', {
                        name: splitPath(first).name,
                      })}
                      className="flex flex-col gap-2"
                    >
                      <p className="text-caption text-muted">
                        {t('tidy.dup.groupInfo', {
                          copies: tn('count.copy', group.files.length),
                          size: formatSize(group.sizeBytes),
                          wasted: formatSize(group.wastedBytes),
                        })}
                        {group.hardLinks > 0
                          ? ` ${t('tidy.dup.hardLinks', { count: group.hardLinks })}`
                          : ''}
                      </p>
                      <ul className="flex flex-col divide-y divide-line">
                        {group.files.map((file, at) => {
                          const { name, folder } = splitPath(file.path);
                          const isMarked = marked?.has(at) ?? false;
                          const allowed = canMark(marks, group, isMarked);
                          return (
                            <li key={file.path} className="flex items-center gap-3 py-2">
                              <Checkbox
                                label={t('tidy.dup.remove', { name })}
                                checked={isMarked}
                                disabled={busy || !allowed}
                                title={allowed ? undefined : t('tidy.dup.lastCopy')}
                                onChange={(on) => setMarks(withMark(marks, group.index, at, on))}
                              />
                              <div className="min-w-0 flex-1">
                                <p className="truncate font-semibold" title={file.path}>
                                  {name}
                                </p>
                                <p className="truncate text-caption text-muted" title={folder}>
                                  {folder}
                                </p>
                                <p className="text-caption text-muted">
                                  {file.modifiedMs === null
                                    ? t('large.row.unknownDate')
                                    : t('large.row.modified', {
                                        date: formatDate(file.modifiedMs),
                                      })}
                                </p>
                              </div>
                              <Button
                                size="sm"
                                disabled={busy}
                                aria-label={t('tidy.dup.keepAria', { name })}
                                onClick={() => setMarks(keepOnly(marks, group, at))}
                              >
                                {t('tidy.dup.keep')}
                              </Button>
                              <Button
                                size="sm"
                                variant="ghost"
                                disabled={busy}
                                aria-label={t('tidy.row.open', { name })}
                                title={t('tidy.row.open', { name })}
                                icon={<FolderOpen aria-hidden size={16} />}
                                onClick={() => open('duplicate', group.index, at)}
                              />
                            </li>
                          );
                        })}
                      </ul>
                    </Card>
                  );
                })}
                <Card as="section" className="flex flex-col gap-3">
                  <p>
                    {totals.copies === 0
                      ? t('tidy.dup.selection.none')
                      : t('tidy.dup.selection.some', {
                          selection: copiesLabel(totals.copies, totals.bytes),
                        })}
                  </p>
                  <div>
                    <Button
                      variant="primary"
                      disabled={totals.copies === 0 || busy}
                      loading={phase === 'moving' && pending === null && totals.copies > 0}
                      onClick={() => setPending('duplicates')}
                    >
                      {t('tidy.move')}
                    </Button>
                  </div>
                </Card>
              </>
            )}
          </Section>

          <Section title={t('tidy.folders.title')} hint={t('tidy.folders.hint')}>
            {lists.emptyFolders.length === 0 ? (
              <Card className="text-center text-muted">{t('tidy.folders.none')}</Card>
            ) : (
              <>
                <Card className="p-0">
                  <ul className="flex flex-col divide-y divide-line">
                    {lists.emptyFolders.map((folder) => {
                      const { name, folder: parent } = splitPath(folder.path);
                      return (
                        <li key={folder.path} className="flex items-center gap-3 px-4 py-3">
                          <Checkbox
                            label={t('tidy.row.select', { name })}
                            checked={folders.has(folder.index)}
                            disabled={busy}
                            onChange={(on) => toggle(setFolders, folder.index, on)}
                          />
                          <div className="min-w-0 flex-1">
                            <p className="truncate font-semibold" title={folder.path}>
                              {name}
                            </p>
                            <p className="truncate text-caption text-muted" title={parent}>
                              {parent}
                            </p>
                            {folder.nested > 0 ? (
                              <p className="text-caption text-muted">
                                {t('tidy.folders.nested', {
                                  folders: tn('count.folder', folder.nested),
                                })}
                              </p>
                            ) : null}
                          </div>
                          <Button
                            size="sm"
                            variant="ghost"
                            disabled={busy}
                            aria-label={t('tidy.row.open', { name })}
                            title={t('tidy.row.open', { name })}
                            icon={<FolderOpen aria-hidden size={16} />}
                            onClick={() => open('emptyFolder', folder.index)}
                          />
                        </li>
                      );
                    })}
                  </ul>
                </Card>
                <Card as="section" className="flex flex-col gap-3">
                  <p>
                    {chosenFolders.length === 0
                      ? t('tidy.folders.selection.none')
                      : t('tidy.selection.some', {
                          selection: tn('count.folder', chosenFolders.length),
                        })}
                  </p>
                  <div className="flex flex-wrap gap-2">
                    <Button
                      disabled={busy}
                      onClick={() => setFolders(new Set(lists.emptyFolders.map((f) => f.index)))}
                    >
                      {t('tidy.selectAll')}
                    </Button>
                    <Button
                      variant="primary"
                      disabled={chosenFolders.length === 0 || busy}
                      onClick={() => setPending('folders')}
                    >
                      {t('tidy.move')}
                    </Button>
                  </div>
                </Card>
              </>
            )}
          </Section>

          <Section title={t('tidy.shortcuts.title')} hint={t('tidy.shortcuts.hint')}>
            {lists.brokenShortcuts.length === 0 ? (
              <Card className="text-center text-muted">{t('tidy.shortcuts.none')}</Card>
            ) : (
              <>
                <Card className="p-0">
                  <ul className="flex flex-col divide-y divide-line">
                    {lists.brokenShortcuts.map((shortcut) => {
                      const { name, folder } = splitPath(shortcut.path);
                      return (
                        <li key={shortcut.path} className="flex items-center gap-3 px-4 py-3">
                          <Checkbox
                            label={t('tidy.row.select', { name })}
                            checked={shortcuts.has(shortcut.index)}
                            disabled={busy}
                            onChange={(on) => toggle(setShortcuts, shortcut.index, on)}
                          />
                          <div className="min-w-0 flex-1">
                            <p className="truncate font-semibold" title={shortcut.path}>
                              {name}
                            </p>
                            <p className="truncate text-caption text-muted" title={folder}>
                              {folder}
                            </p>
                            <p
                              className="wrap-anywhere text-caption text-muted"
                              title={shortcut.target}
                            >
                              {t('tidy.shortcuts.target', { target: shortcut.target })}
                            </p>
                          </div>
                          <Button
                            size="sm"
                            variant="ghost"
                            disabled={busy}
                            aria-label={t('tidy.row.open', { name })}
                            title={t('tidy.row.open', { name })}
                            icon={<FolderOpen aria-hidden size={16} />}
                            onClick={() => open('shortcut', shortcut.index)}
                          />
                        </li>
                      );
                    })}
                  </ul>
                </Card>
                <Card as="section" className="flex flex-col gap-3">
                  <p>
                    {chosenShortcuts.length === 0
                      ? t('tidy.shortcuts.selection.none')
                      : t('tidy.selection.some', {
                          selection: tn('count.shortcut', chosenShortcuts.length),
                        })}
                  </p>
                  <div className="flex flex-wrap gap-2">
                    <Button
                      disabled={busy}
                      onClick={() =>
                        setShortcuts(new Set(lists.brokenShortcuts.map((s) => s.index)))
                      }
                    >
                      {t('tidy.selectAll')}
                    </Button>
                    <Button
                      variant="primary"
                      disabled={chosenShortcuts.length === 0 || busy}
                      onClick={() => setPending('shortcuts')}
                    >
                      {t('tidy.move')}
                    </Button>
                  </div>
                </Card>
              </>
            )}
          </Section>
        </>
      ) : null}

      <Dialog
        open={pending !== null}
        onClose={() => setPending(null)}
        title={t('tidy.confirm.title')}
        description={`${t('tidy.confirm.description', { selection: pendingLabel })}${
          pending === 'duplicates' ? ` ${t('tidy.confirm.keeps')}` : ''
        }`}
        footer={
          <>
            <Button onClick={() => setPending(null)}>{t('common.cancel')}</Button>
            <Button variant="primary" onClick={() => void move()}>
              {t('tidy.move')}
            </Button>
          </>
        }
      >
        <ul aria-label={t('tidy.confirm.list')} className="flex flex-col gap-1 text-caption">
          {pendingRows.slice(0, PREVIEW_ROWS).map((row) => (
            <li key={row.path} className="flex gap-2">
              <span className="min-w-0 flex-1 wrap-anywhere font-mono">{row.path}</span>
              {row.sizeBytes === null ? null : (
                <span className="shrink-0 text-muted">{formatSize(row.sizeBytes)}</span>
              )}
            </li>
          ))}
          {pendingRows.length > PREVIEW_ROWS ? (
            <li className="text-muted">
              {t('tidy.confirm.more', { count: pendingRows.length - PREVIEW_ROWS })}
            </li>
          ) : null}
        </ul>
      </Dialog>
    </div>
  );
}
