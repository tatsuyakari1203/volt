import { Loader2, Sparkles, TriangleAlert } from 'lucide-react';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
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
import { Button, Card, Dialog, ListGroup, PageHeader } from '../../shared/ui';
import { countOf, formatSize } from './format';
import { describeError, selectionOf, selectionText, tickedByDefault } from './messages';
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
    else groups.push({ key: rule.category, label: rule.categoryLabel, rules: [rule] });
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
        Dismiss
      </button>
    </div>
  );
}

function ResultCard({ result, onScanAgain }: { result: CleanRunResult; onScanAgain: () => void }) {
  return (
    <Card as="section" aria-label="Clean result" className="flex flex-col gap-3">
      <h2 className="font-semibold">Moved to the Recycle Bin</h2>
      <p>{result.message}</p>
      {result.rules.length > 0 ? (
        <ul className="flex flex-col gap-1 text-caption text-muted">
          {result.rules.map((rule) => (
            <li key={rule.id}>
              {rule.title}: {countOf(rule.movedFiles, 'file')} ({formatSize(rule.movedBytes)})
              {rule.skipped > 0 ? `, ${countOf(rule.skipped, 'item')} left in place` : ''}
            </li>
          ))}
        </ul>
      ) : null}
      {result.skipped.length > 0 ? (
        <div className="text-caption text-muted">
          <p className="font-semibold">Left in place</p>
          <ul>
            {result.skipped.map((group) => (
              <li key={group.reason}>
                {countOf(group.count, 'item')}: {group.reason}
              </li>
            ))}
          </ul>
        </div>
      ) : null}
      {result.historyIncomplete ? (
        <p className="text-caption text-muted">
          The activity history could not be written, so this clean is missing from it.
        </p>
      ) : null}
      <div>
        <Button onClick={onScanAgain}>Scan again</Button>
      </div>
    </Card>
  );
}

/**
 * The Clean screen: scan, see per rule how much would go, untick what you want to keep, confirm,
 * and read an honest result. Files only ever move to the Recycle Bin, which keeps their space until
 * it is emptied: the separate card below does that, for good, after its own confirmation. All
 * rules live in the Rust core; this renders what the commands report.
 */
export function CleanScreen() {
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
        setNotice(`The scan could not run. ${describeError(error)}`);
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
      setNotice(`Nothing was moved. ${describeError(error)}`);
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
      <PageHeader
        title="Clean"
        description="Find junk files and send them to the Recycle Bin. Nothing is deleted for good until you empty the Recycle Bin."
      />
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
                  ? 'Stopping after this rule...'
                  : progress
                    ? `Checking rule ${Math.min(progress.done + 1, progress.total)} of ${progress.total}...`
                    : 'Starting the scan...'}
              </p>
              <Button size="sm" disabled={cancelling} onClick={cancelScan}>
                Cancel
              </Button>
            </div>
            <div
              role="progressbar"
              aria-label="Scan progress"
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
            <p className="text-caption text-muted">
              Looking only. Nothing is changed while Volt scans.
            </p>
          </Card>
        ) : null}

        {result && !scanning ? (
          <ResultCard result={result} onScanAgain={() => void startScan()} />
        ) : null}

        {!scan && !result && !scanning && rulesLoad.status === 'ready' ? (
          <Card className="flex flex-col items-center gap-3 py-8 text-center">
            <Sparkles aria-hidden size={24} className="text-muted" />
            <p className="font-semibold">Scan for junk files</p>
            <p className="max-w-xs text-body text-muted">
              Volt looks, and lists what it finds with sizes. You choose what goes before anything
              moves.
            </p>
            <Button variant="primary" onClick={() => void startScan()}>
              Scan
            </Button>
          </Card>
        ) : null}

        {scan && !scanning ? (
          <Card as="section" aria-label="Selection" className="flex flex-col gap-3">
            <p>
              {nothingSelected ? (
                'Nothing is selected to move.'
              ) : (
                <>
                  <span className="font-semibold">{selectionText(selection)}</span> selected. They
                  will be moved to the Recycle Bin, not deleted.
                </>
              )}
            </p>
            <div className="flex flex-wrap gap-2">
              <Button
                variant="primary"
                disabled={nothingSelected}
                loading={cleaning}
                onClick={() => setConfirming(true)}
              >
                Clean selected
              </Button>
              <Button disabled={cleaning} onClick={() => void startScan()}>
                Scan again
              </Button>
            </div>
          </Card>
        ) : null}

        {rulesLoad.status === 'loading' ? (
          <Card role="status" className="flex items-center justify-center gap-2 py-10 text-muted">
            <Loader2 aria-hidden size={18} className="animate-spin motion-reduce:animate-none" />
            <p>Reading the cleaning rules...</p>
          </Card>
        ) : null}

        {rulesLoad.status === 'error' ? (
          <Card role="alert" className="flex flex-col items-center gap-3 py-10 text-center">
            <TriangleAlert aria-hidden size={24} className="text-muted" />
            <p className="font-semibold">The cleaning rules could not be loaded</p>
            <p className="max-w-xs text-body text-muted">{rulesLoad.message}</p>
            <Button onClick={readRules}>Try again</Button>
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

        <RecycleBinCard refreshKey={binKey} />
      </div>

      <Dialog
        open={confirming}
        onClose={() => setConfirming(false)}
        title="Move these files to the Recycle Bin?"
        description={`${selectionText(selection)} will be moved to the Recycle Bin. Nothing is deleted for good, and the files keep using disk space until you empty the Recycle Bin. You can restore them from there.`}
        footer={
          <>
            <Button onClick={() => setConfirming(false)}>Cancel</Button>
            <Button variant="primary" onClick={() => void clean()}>
              Move to Recycle Bin
            </Button>
          </>
        }
      />
    </>
  );
}
