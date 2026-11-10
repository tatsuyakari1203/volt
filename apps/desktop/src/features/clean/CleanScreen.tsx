import { Loader2, Sparkles, TriangleAlert } from 'lucide-react';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { t, tNodes, tn } from '../../shared/i18n';
import {
  type CleaningRuleInfo,
  type CleanRunResult,
  type CleanScanResult,
  cancelCleaningScan,
  cleanSelectedRules,
  IpcCommandError,
  listCleaningRules,
  onCleanScanProgress,
  scanCleaning,
} from '../../shared/ipc';
import { formatSize } from '../../shared/text/size';
import { Button, Card, Dialog, ListGroup, PageHeader, SegmentedControl } from '../../shared/ui';
import { LargeFilesView } from './LargeFilesView';
import {
  categoryName,
  describeError,
  resultMessage,
  ruleTitle,
  selectionOf,
  selectionText,
  tickedByDefault,
} from './messages';
import { RecycleBinCard } from './RecycleBinCard';
import { RuleRow } from './RuleRow';

type RulesLoad =
  | { status: 'loading' }
  | { status: 'error'; message: string }
  | { status: 'ready'; rules: CleaningRuleInfo[] };

type Phase = 'idle' | 'scanning' | 'ready' | 'cleaning';

/** Rules grouped by category, in the order the categories first appear. */
function groupByCategory(rules: readonly CleaningRuleInfo[]) {
  const groups: { key: string; label: string; rules: CleaningRuleInfo[] }[] = [];
  for (const rule of rules) {
    const group = groups.find((candidate) => candidate.key === rule.category);
    if (group) group.rules.push(rule);
    else groups.push({ key: rule.category, label: categoryName(rule), rules: [rule] });
  }
  return groups;
}

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

function ResultCard({ result, onScanAgain }: { result: CleanRunResult; onScanAgain: () => void }) {
  return (
    <Card as="section" aria-label={t('clean.result.label')} className="flex flex-col gap-3">
      <h2 className="font-semibold">{t('clean.result.title')}</h2>
      <p>{resultMessage(result)}</p>
      {result.rules.length > 0 ? (
        <ul className="flex flex-col gap-1 text-caption text-muted">
          {result.rules.map((rule) => (
            <li key={rule.id}>
              {t(rule.skipped > 0 ? 'clean.result.ruleLeft' : 'clean.result.rule', {
                title: ruleTitle(rule.id, rule.title),
                files: tn('count.file', rule.movedFiles),
                size: formatSize(rule.movedBytes),
                items: tn('count.item', rule.skipped),
              })}
            </li>
          ))}
        </ul>
      ) : null}
      {result.skipped.length > 0 ? (
        <div className="text-caption text-muted">
          <p className="font-semibold">{t('clean.result.leftInPlace')}</p>
          <ul>
            {result.skipped.map((group) => (
              <li key={group.reason}>
                {t('clean.result.group', {
                  items: tn('count.item', group.count),
                  reason: group.reason,
                })}
              </li>
            ))}
          </ul>
        </div>
      ) : null}
      {result.historyIncomplete ? (
        <p className="text-caption text-muted">{t('clean.result.historyIncomplete')}</p>
      ) : null}
      <div>
        <Button onClick={onScanAgain}>{t('clean.scanAgain')}</Button>
      </div>
    </Card>
  );
}

/**
 * The junk view of the Clean screen: scan, see per rule how much would go, untick what you want to keep, confirm,
 * and read an honest result. Files only ever move to the Recycle Bin, which keeps their space until
 * it is emptied: the separate card below does that, for good, after its own confirmation. All
 * rules live in the Rust core; this renders what the commands report.
 */
function JunkClean({ movedElsewhere }: { movedElsewhere: number }) {
  const [rulesLoad, setRulesLoad] = useState<RulesLoad>({ status: 'loading' });
  const [phase, setPhase] = useState<Phase>('idle');
  const [scan, setScan] = useState<CleanScanResult | null>(null);
  const [ticked, setTicked] = useState<ReadonlySet<string>>(new Set());
  const [progress, setProgress] = useState<{ done: number; total: number } | null>(null);
  const [cancelling, setCancelling] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const [result, setResult] = useState<CleanRunResult | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [binKey, setBinKey] = useState(0);
  const alive = useRef(true);

  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  const readRules = useCallback(() => {
    setRulesLoad({ status: 'loading' });
    listCleaningRules()
      .then((rules) => {
        if (alive.current) setRulesLoad({ status: 'ready', rules });
      })
      .catch((error: unknown) => {
        if (alive.current) setRulesLoad({ status: 'error', message: describeError(error) });
      });
  }, []);

  useEffect(() => readRules(), [readRules]);

  const startScan = async () => {
    setPhase('scanning');
    setProgress(null);
    setCancelling(false);
    setNotice(null);
    setResult(null);
    let unlisten: (() => void) | undefined;
    try {
      unlisten = await onCleanScanProgress((next) => {
        if (alive.current) setProgress(next);
      });
    } catch {
      // No progress without the event bridge; the scan itself still works.
    }
    try {
      const found = await scanCleaning();
      if (!alive.current) return;
      if (found.cancelled) {
        setPhase('idle');
        return;
      }
      setScan(found);
      setTicked(new Set(found.rules.filter((r) => tickedByDefault(r.rule)).map((r) => r.rule.id)));
      setPhase('idle');
    } catch (error) {
      if (alive.current) {
        setNotice(t('clean.scan.failed', { error: describeError(error) }));
        setPhase('idle');
      }
    } finally {
      unlisten?.();
    }
  };

  const cancelScan = () => {
    setCancelling(true);
    cancelCleaningScan().catch(() => {});
  };

  const toggle = (id: string, on: boolean) => {
    setTicked((current) => {
      const next = new Set(current);
      if (on) next.add(id);
      else next.delete(id);
      return next;
    });
  };

  const clean = async () => {
    if (!scan) return;
    setConfirming(false);
    setPhase('cleaning');
    setNotice(null);
    try {
      const done = await cleanSelectedRules(scan.planId, [...ticked]);
      if (!alive.current) return;
      setResult(done);
      // The scan is used up: what it listed has moved, so a new scan is needed to clean again.
      setScan(null);
      setTicked(new Set());
      setBinKey((key) => key + 1);
    } catch (error) {
      if (!alive.current) return;
      setNotice(t('clean.move.failed', { error: describeError(error) }));
      if (error instanceof IpcCommandError && error.error.kind === 'notFound') setScan(null);
    } finally {
      if (alive.current) setPhase('idle');
    }
  };

  const rules = rulesLoad.status === 'ready' ? rulesLoad.rules : [];
  const views = useMemo(() => scan?.rules ?? [], [scan]);
  const listed = useMemo(
    () => (scan ? views.map((view) => view.rule) : rules),
    [scan, views, rules],
  );
  const groups = useMemo(() => groupByCategory(listed), [listed]);
  const viewById = useMemo(() => new Map(views.map((view) => [view.rule.id, view])), [views]);
  const selection = useMemo(() => selectionOf(views, ticked), [views, ticked]);
  const nothingSelected = selection.files + selection.folders === 0;
  const scanning = phase === 'scanning';
  const cleaning = phase === 'cleaning';

  return (
    <>
      <div className="flex flex-col gap-3">
        {notice ? <Notice message={notice} onDismiss={() => setNotice(null)} /> : null}

        {scanning ? (
          <Card role="status" className="flex flex-col gap-3">
            <div className="flex items-center gap-2">
              <Loader2
                aria-hidden
                size={18}
                className="shrink-0 animate-spin motion-reduce:animate-none"
              />
              <p className="min-w-0 flex-1">
                {cancelling
                  ? t('clean.scan.stopping')
                  : progress
                    ? t('clean.scan.progress', {
                        current: Math.min(progress.done + 1, progress.total),
                        total: progress.total,
                      })
                    : t('clean.scan.starting')}
              </p>
              <Button size="sm" disabled={cancelling} onClick={cancelScan}>
                {t('common.cancel')}
              </Button>
            </div>
            <div
              role="progressbar"
              aria-label={t('clean.scan.progressLabel')}
              aria-valuemin={0}
              aria-valuemax={progress?.total ?? 1}
              aria-valuenow={progress?.done ?? 0}
              className="h-1.5 overflow-hidden rounded-full bg-surface-sunken"
            >
              <div
                className="h-full bg-accent transition-[width] duration-(--duration-base) ease-standard"
                style={{
                  width: `${progress && progress.total > 0 ? (progress.done / progress.total) * 100 : 0}%`,
                }}
              />
            </div>
            <p className="text-caption text-muted">{t('clean.scan.readonly')}</p>
          </Card>
        ) : null}

        {result && !scanning ? (
          <ResultCard result={result} onScanAgain={() => void startScan()} />
        ) : null}

        {!scan && !result && !scanning && rulesLoad.status === 'ready' ? (
          <Card className="flex flex-col items-center gap-3 py-8 text-center">
            <Sparkles aria-hidden size={24} className="text-muted" />
            <p className="font-semibold">{t('clean.empty.title')}</p>
            <p className="max-w-xs text-body text-muted">{t('clean.empty.description')}</p>
            <Button variant="primary" onClick={() => void startScan()}>
              {t('clean.scan.button')}
            </Button>
          </Card>
        ) : null}

        {scan && !scanning ? (
          <Card
            as="section"
            aria-label={t('clean.selection.label')}
            className="flex flex-col gap-3"
          >
            <p>
              {nothingSelected
                ? t('clean.selection.none')
                : tNodes('clean.selection.some', {
                    selection: <span className="font-semibold">{selectionText(selection)}</span>,
                  })}
            </p>
            <div className="flex flex-wrap gap-2">
              <Button
                variant="primary"
                disabled={nothingSelected}
                loading={cleaning}
                onClick={() => setConfirming(true)}
              >
                {t('clean.cleanSelected')}
              </Button>
              <Button disabled={cleaning} onClick={() => void startScan()}>
                {t('clean.scanAgain')}
              </Button>
            </div>
          </Card>
        ) : null}

        {rulesLoad.status === 'loading' ? (
          <Card role="status" className="flex items-center justify-center gap-2 py-10 text-muted">
            <Loader2 aria-hidden size={18} className="animate-spin motion-reduce:animate-none" />
            <p>{t('clean.rules.loading')}</p>
          </Card>
        ) : null}

        {rulesLoad.status === 'error' ? (
          <Card role="alert" className="flex flex-col items-center gap-3 py-10 text-center">
            <TriangleAlert aria-hidden size={24} className="text-muted" />
            <p className="font-semibold">{t('clean.rules.error')}</p>
            <p className="max-w-xs text-body text-muted">{rulesLoad.message}</p>
            <Button onClick={readRules}>{t('common.tryAgain')}</Button>
          </Card>
        ) : null}

        {!scanning && listed.length > 0
          ? groups.map((group) => (
              <section key={group.key} aria-label={group.label} className="flex flex-col gap-2">
                <h2 className="text-caption font-semibold text-muted">{group.label}</h2>
                <ListGroup aria-label={group.label}>
                  {group.rules.map((rule) => (
                    <RuleRow
                      key={rule.id}
                      rule={rule}
                      view={viewById.get(rule.id)}
                      checked={ticked.has(rule.id)}
                      enabled={!cleaning}
                      onCheckedChange={(on) => toggle(rule.id, on)}
                    />
                  ))}
                </ListGroup>
              </section>
            ))
          : null}

        <RecycleBinCard refreshKey={binKey + movedElsewhere} />
      </div>

      <Dialog
        open={confirming}
        onClose={() => setConfirming(false)}
        title={t('clean.confirm.title')}
        description={t('clean.confirm.description', { selection: selectionText(selection) })}
        footer={
          <>
            <Button onClick={() => setConfirming(false)}>{t('common.cancel')}</Button>
            <Button variant="primary" onClick={() => void clean()}>
              {t('clean.confirm.move')}
            </Button>
          </>
        }
      />
    </>
  );
}

type View = 'junk' | 'large';

/**
 * The Clean screen: a switch between the junk scan and the large files finder. Both stay mounted
 * (the inactive one is hidden), so a scan result survives switching views. A move in the large
 * files view makes the Recycle Bin card of the junk view read the bin again.
 */
export function CleanScreen() {
  const [view, setView] = useState<View>('junk');
  const [moves, setMoves] = useState(0);
  return (
    <>
      <PageHeader title={t('nav.clean')} description={t('clean.description')} />
      <div className="mb-3">
        <SegmentedControl
          label={t('clean.view.label')}
          value={view}
          onValueChange={setView}
          options={[
            { value: 'junk', label: t('clean.view.junk') },
            { value: 'large', label: t('clean.view.large') },
          ]}
        />
      </div>
      <div hidden={view !== 'junk'}>
        <JunkClean movedElsewhere={moves} />
      </div>
      <div hidden={view !== 'large'}>
        <LargeFilesView onMoved={() => setMoves((count) => count + 1)} />
      </div>
    </>
  );
}
