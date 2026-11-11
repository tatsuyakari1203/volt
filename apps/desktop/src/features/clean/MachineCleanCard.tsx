import { Loader2, ShieldAlert, ShieldCheck, TriangleAlert } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { say, t } from '../../shared/i18n';
import {
  type CleaningRuleInfo,
  cleanMachineRule,
  type ElevationStatusView,
  getElevationStatus,
  type MachineCleanResult,
  type MachineScanView,
  scanMachineRule,
} from '../../shared/ipc';
import { elevationReasonText } from '../../shared/text/memory';
import { Badge, Button, Card, Dialog, ListGroup } from '../../shared/ui';
import { describeError, ruleTitle, selectionText } from './messages';

/** What a delete answered, kept until the next scan. */
interface Outcome {
  id: string;
  title: string;
  result: MachineCleanResult;
}

/** What would go, in words: files, empty folders and size. */
function whatText(view: MachineScanView): string {
  return selectionText({ files: view.files, folders: view.folders, bytes: view.bytes });
}

/** The line a refusal or an outcome that deleted nothing is written as. */
function refusalText(result: Exclude<MachineCleanResult, { outcome: 'done' }>): string {
  switch (result.outcome) {
    case 'declined':
      return t('clean.machine.result.declined');
    case 'notConfirmed':
      return t('clean.machine.result.notConfirmed');
    case 'blocked':
      return t('clean.machine.result.blocked');
    case 'timedOut':
      return t('clean.machine.result.timedOut');
    case 'noScan':
      return t('clean.machine.result.noScan');
  }
}

function OutcomeLines({ outcome }: { outcome: Outcome }) {
  const { result, title } = outcome;
  if (result.outcome !== 'done') {
    return (
      <li>
        <span className="font-semibold">{title}: </span>
        {refusalText(result)}
      </li>
    );
  }
  return (
    <li className="flex flex-col gap-0.5">
      <p>
        <span className="font-semibold">{title}: </span>
        {say(result.messagePhrase, result.message)}
      </p>
      {result.skippedTotal > 0 && result.skipped ? (
        <p className="text-caption text-muted">
          {t('clean.machine.result.left', { reason: say(result.skippedPhrase, result.skipped) })}
        </p>
      ) : null}
      {result.stopped ? (
        <p className="text-caption text-muted">{t('clean.machine.result.stopped')}</p>
      ) : null}
      {result.historyIncomplete ? (
        <p className="text-caption text-muted">{t('clean.result.historyIncomplete')}</p>
      ) : null}
    </li>
  );
}

/**
 * The machine-wide cleaning rules, cleaned by the administrator helper: Windows Temp, the error
 * report folders for every account and the Windows Update logs. A scan shows counts and sizes and
 * never a file name. A delete is PERMANENT (nothing goes to the Recycle Bin and nothing can be
 * undone), is confirmed here in those words, and for a folder a normal user can write to the helper
 * asks once more on its own surface. Without the helper (a per-user install, a standard user) the
 * card says why and offers nothing. All rules live in the Rust core and the helper.
 */
export function MachineCleanCard({ rules }: { rules: readonly CleaningRuleInfo[] }) {
  // `null` until it is known, and when it could not be read: administrator actions stay locked.
  const [elevation, setElevation] = useState<ElevationStatusView | null>(null);
  const [views, setViews] = useState<ReadonlyMap<string, MachineScanView>>(new Map());
  const [scanning, setScanning] = useState(false);
  const [deleting, setDeleting] = useState<string | null>(null);
  const [confirming, setConfirming] = useState<CleaningRuleInfo | null>(null);
  const [outcomes, setOutcomes] = useState<Outcome[]>([]);
  const [notice, setNotice] = useState<string | null>(null);
  const alive = useRef(true);

  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  useEffect(() => {
    getElevationStatus()
      .then((status) => {
        if (alive.current) setElevation(status);
      })
      .catch(() => {
        if (alive.current) setElevation(null);
      });
  }, []);

  if (rules.length === 0) return null;
  const available = elevation?.state === 'available';
  const busy = scanning || deleting !== null;

  const scan = async () => {
    setScanning(true);
    setNotice(null);
    setOutcomes([]);
    try {
      const found = new Map<string, MachineScanView>();
      for (const rule of rules) {
        const answer = await scanMachineRule(rule.id);
        if (!alive.current) return;
        if (answer.outcome === 'declined') {
          setNotice(t('clean.machine.result.declined'));
          break;
        }
        found.set(rule.id, answer.scan);
      }
      setViews(found);
    } catch (error) {
      if (alive.current) setNotice(t('clean.machine.failed', { error: describeError(error) }));
    } finally {
      if (alive.current) setScanning(false);
    }
  };

  const remove = async (rule: CleaningRuleInfo) => {
    setConfirming(null);
    setDeleting(rule.id);
    setNotice(null);
    try {
      const result = await cleanMachineRule(rule.id);
      if (!alive.current) return;
      setOutcomes((before) => [
        ...before.filter((outcome) => outcome.id !== rule.id),
        { id: rule.id, title: ruleTitle(rule.id, rule.title), result },
      ]);
      // The scan is used up whatever happened: delete only after a new one.
      setViews((before) => {
        const next = new Map(before);
        next.delete(rule.id);
        return next;
      });
    } catch (error) {
      if (alive.current)
        setNotice(t('clean.machine.deleteFailed', { error: describeError(error) }));
    } finally {
      if (alive.current) setDeleting(null);
    }
  };

  const confirmView = confirming ? views.get(confirming.id) : undefined;
  return (
    <>
      <Card as="section" aria-label={t('clean.machine.title')} className="flex flex-col gap-3">
        <div className="flex flex-wrap items-center gap-2">
          <h2 className="min-w-0 flex-1 font-semibold">{t('clean.machine.title')}</h2>
          <Button
            size="sm"
            variant={views.size === 0 ? 'primary' : 'secondary'}
            icon={
              scanning ? (
                <Loader2
                  aria-hidden
                  size={16}
                  className="animate-spin motion-reduce:animate-none"
                />
              ) : (
                <ShieldCheck aria-hidden size={16} />
              )
            }
            disabled={!available || busy}
            onClick={() => void scan()}
          >
            {scanning
              ? t('clean.machine.scanning')
              : views.size === 0
                ? t('clean.machine.scan')
                : t('clean.machine.rescan')}
          </Button>
        </div>
        <p className="text-body text-muted">{t('clean.machine.description')}</p>
        {elevation?.state === 'unavailable' ? (
          <p role="note" className="flex items-start gap-2 text-body font-semibold">
            <ShieldAlert aria-hidden size={18} className="mt-0.5 shrink-0" />
            {elevationReasonText(elevation.reason)}
          </p>
        ) : null}
        {notice ? (
          <div
            role="alert"
            className="flex items-start gap-3 rounded-md bg-caution p-3 text-body text-caution-fg"
          >
            <TriangleAlert aria-hidden size={18} className="mt-0.5 shrink-0" />
            <p className="min-w-0 flex-1">{notice}</p>
            <button
              type="button"
              className="shrink-0 rounded-sm px-2 py-1 font-semibold underline hover:no-underline"
              onClick={() => setNotice(null)}
            >
              {t('common.dismiss')}
            </button>
          </div>
        ) : null}

        <ListGroup aria-label={t('clean.machine.title')}>
          {rules.map((rule) => {
            const view = views.get(rule.id);
            const title = ruleTitle(rule.id, rule.title);
            return (
              <li key={rule.id} className="px-4 py-3">
                <div className="flex items-center gap-3">
                  <div className="min-w-0 flex-1">
                    <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
                      <span className="font-semibold">{title}</span>
                      <Badge risk={rule.risk} />
                    </div>
                    <p className="mt-0.5 text-caption text-muted">
                      {view
                        ? view.problem
                          ? say(view.problemPhrase, view.problem)
                          : view.files + view.folders === 0
                            ? t('clean.machine.nothing')
                            : t('clean.machine.found', { what: whatText(view) })
                        : t('clean.machine.notScanned')}
                      {view?.truncated ? ` ${t('clean.rule.truncated')}` : ''}
                    </p>
                    {view?.skipped ? (
                      <p className="text-caption text-muted">
                        {t('clean.rule.leftOut', { reason: say(view.skippedPhrase, view.skipped) })}
                      </p>
                    ) : null}
                    {view?.canClean && view.confirm ? (
                      <p className="text-caption text-muted">{t('clean.machine.confirmsAgain')}</p>
                    ) : null}
                  </div>
                  {view?.canClean ? (
                    <Button
                      size="sm"
                      aria-label={t('clean.machine.deleteFor', { title })}
                      loading={deleting === rule.id}
                      disabled={busy && deleting !== rule.id}
                      onClick={() => setConfirming(rule)}
                    >
                      {deleting === rule.id
                        ? t('clean.machine.deleting')
                        : t('clean.machine.delete')}
                    </Button>
                  ) : null}
                </div>
              </li>
            );
          })}
        </ListGroup>

        {outcomes.length > 0 ? (
          <div role="status" className="flex flex-col gap-1">
            <h3 className="text-caption font-semibold text-muted">
              {t('clean.machine.result.title')}
            </h3>
            <ul className="flex flex-col gap-2 text-body">
              {outcomes.map((outcome) => (
                <OutcomeLines key={outcome.id} outcome={outcome} />
              ))}
            </ul>
          </div>
        ) : null}
      </Card>

      <Dialog
        open={confirming !== null && confirmView !== undefined}
        onClose={() => setConfirming(null)}
        title={t('clean.machine.confirm.title')}
        description={
          confirming && confirmView
            ? t('clean.machine.confirm.description', {
                title: ruleTitle(confirming.id, confirming.title),
                what: whatText(confirmView),
              })
            : ''
        }
        footer={
          <>
            <Button onClick={() => setConfirming(null)}>{t('common.cancel')}</Button>
            <Button variant="primary" onClick={() => confirming && void remove(confirming)}>
              {t('clean.machine.delete')}
            </Button>
          </>
        }
      >
        {confirmView?.confirm ? (
          <p role="note" className="text-body text-muted">
            {t('clean.machine.confirm.askAgain')}
          </p>
        ) : null}
      </Dialog>
    </>
  );
}
