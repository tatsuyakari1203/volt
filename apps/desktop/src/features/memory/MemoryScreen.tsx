import {
  CircleCheck,
  Loader2,
  RefreshCw,
  ShieldAlert,
  ShieldCheck,
  TriangleAlert,
} from 'lucide-react';
import { useCallback, useEffect, useRef, useState } from 'react';
import { t, tNodes } from '../../shared/i18n';
import {
  cleanMemory,
  cleanMemoryElevated,
  type ElevationStatusView,
  getElevationStatus,
  getMemoryState,
  type MemoryAreaView,
  type MemoryCleanResult,
  type MemoryListsView,
  type MemoryState,
} from '../../shared/ipc';
import {
  describeArea,
  describeError,
  elevationReasonText,
  formatChange,
  listRows,
  operationText,
  reasonText,
} from '../../shared/text/memory';
import { formatBytes } from '../../shared/text/size';
import { Badge, Button, Card, ListGroup, ListRow, PageHeader } from '../../shared/ui';

type Load =
  | { status: 'loading' }
  | { status: 'error'; message: string }
  | { status: 'ready'; state: MemoryState };

function share(part: number, total: number): number {
  return total > 0 ? (part / total) * 100 : 0;
}

function ListsCard({ lists }: { lists: MemoryListsView }) {
  const summary = listRows
    .map((row) => `${t(row.label)} ${formatBytes(lists[row.key])}`)
    .join(', ');
  return (
    <Card as="section" aria-label={t('memory.lists.label')} className="flex flex-col gap-3">
      <p className="text-body">
        {tNodes('memory.installed', {
          size: <span className="font-semibold">{formatBytes(lists.totalBytes)}</span>,
        })}
      </p>
      <div
        role="img"
        aria-label={summary}
        className="flex h-3 overflow-hidden rounded-sm border border-line"
      >
        {listRows.map((row) => (
          <span
            key={row.key}
            className={row.swatch}
            style={{ width: `${share(lists[row.key], lists.totalBytes)}%` }}
          />
        ))}
      </div>
      <dl className="flex flex-col gap-2">
        {listRows.map((row) => (
          <div key={row.key} className="flex items-start gap-2">
            <span
              aria-hidden
              className={`mt-1 size-3 shrink-0 rounded-sm border border-line ${row.swatch}`}
            />
            <div className="min-w-0 flex-1">
              <div className="flex justify-between gap-2">
                <dt className="font-semibold">{t(row.label)}</dt>
                <dd className="tabular-nums">{formatBytes(lists[row.key])}</dd>
              </div>
              <p className="text-caption text-muted">{t(row.hint)}</p>
            </div>
          </div>
        ))}
      </dl>
    </Card>
  );
}

function ReportCard({ report }: { report: MemoryCleanResult }) {
  const { before, after } = report;
  return (
    <Card as="section" aria-label={t('memory.report.title')} className="flex flex-col gap-3">
      <h2 className="text-subtitle font-semibold">{t('memory.report.title')}</h2>
      {before && after ? (
        <table className="w-full text-body">
          <thead>
            <tr className="text-left text-caption text-muted">
              <th scope="col" className="font-semibold">
                {t('memory.report.list')}
              </th>
              <th scope="col" className="text-right font-semibold">
                {t('memory.report.before')}
              </th>
              <th scope="col" className="text-right font-semibold">
                {t('memory.report.after')}
              </th>
              <th scope="col" className="text-right font-semibold">
                {t('memory.report.change')}
              </th>
            </tr>
          </thead>
          <tbody>
            {listRows.map((row) => (
              <tr key={row.key}>
                <th scope="row" className="py-0.5 text-left font-normal">
                  {t(row.label)}
                </th>
                <td className="text-right tabular-nums">{formatBytes(before[row.key])}</td>
                <td className="text-right tabular-nums">{formatBytes(after[row.key])}</td>
                <td className="text-right tabular-nums text-muted">
                  {formatChange(before[row.key], after[row.key])}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      ) : (
        <p className="text-body text-muted">{t('memory.report.noLists')}</p>
      )}
      <ul className="flex flex-col gap-1">
        {report.areas.map((result) => (
          <li key={result.area} className="flex items-start gap-2 text-body">
            {result.status === 'done' ? (
              <CircleCheck aria-hidden size={16} className="mt-0.5 shrink-0" />
            ) : (
              <TriangleAlert aria-hidden size={16} className="mt-0.5 shrink-0" />
            )}
            <span>
              <span className="font-semibold">{operationText(result.area).title}.</span>{' '}
              {describeArea(result)}
            </span>
          </li>
        ))}
      </ul>
      <p className="text-caption text-muted">
        {t('memory.report.footer')}{' '}
        {report.recorded ? t('memory.report.recorded') : t('memory.report.notRecorded')}
      </p>
    </Card>
  );
}

function GuidanceCard() {
  return (
    <Card as="section" aria-label={t('memory.guide.label')} className="flex flex-col gap-2">
      <h2 className="text-subtitle font-semibold">{t('memory.guide.title')}</h2>
      <p className="text-body">
        <span className="font-semibold">{t('memory.guide.helps.lead')}</span>{' '}
        {t('memory.guide.helps.body')}
      </p>
      <p className="text-body">
        <span className="font-semibold">{t('memory.guide.doesNot.lead')}</span>{' '}
        {t('memory.guide.doesNot.body')}
      </p>
    </Card>
  );
}

/**
 * The Memory screen: how memory is used by list (in use, standby, modified, free), one button
 * per cleaning operation and a before and after report for each run. An operation that needs
 * administrator rights shows a shield: when administrator actions are available it runs through
 * the elevated helper (one Windows prompt per session), otherwise it is locked and says why. All
 * rules live in the Rust core: this renders what the commands report.
 */
export function MemoryScreen() {
  const [load, setLoad] = useState<Load>({ status: 'loading' });
  const [running, setRunning] = useState<MemoryAreaView | null>(null);
  const [report, setReport] = useState<MemoryCleanResult | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  // `null` until it is known, and when it could not be read: administrator actions stay locked.
  const [elevation, setElevation] = useState<ElevationStatusView | null>(null);
  const alive = useRef(true);

  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  const read = useCallback((quiet = false) => {
    if (!quiet) setLoad({ status: 'loading' });
    getMemoryState()
      .then((state) => {
        if (alive.current) setLoad({ status: 'ready', state });
      })
      .catch((error: unknown) => {
        if (alive.current) setLoad({ status: 'error', message: describeError(error) });
      });
  }, []);

  useEffect(() => read(), [read]);

  useEffect(() => {
    getElevationStatus()
      .then((status) => {
        if (alive.current) setElevation(status);
      })
      .catch(() => {
        if (alive.current) setElevation(null);
      });
  }, []);

  const run = async (area: MemoryAreaView, asAdministrator: boolean) => {
    setRunning(area);
    setNotice(null);
    try {
      if (asAdministrator) {
        const outcome = await cleanMemoryElevated([area]);
        if (!alive.current) return;
        if (outcome.outcome === 'declined') {
          setNotice(t('memory.declined'));
        } else {
          setReport(outcome.report);
          read(true);
        }
      } else {
        const result = await cleanMemory([area]);
        if (alive.current) {
          setReport(result);
          // Show what is true now, not the numbers from before the run.
          read(true);
        }
      }
    } catch (error) {
      if (alive.current) setNotice(describeError(error));
    } finally {
      if (alive.current) setRunning(null);
    }
  };

  const state = load.status === 'ready' ? load.state : null;
  return (
    <>
      <PageHeader
        title={t('nav.memory')}
        description={t('memory.description')}
        actions={
          <Button
            size="sm"
            icon={<RefreshCw aria-hidden size={16} />}
            disabled={running !== null || load.status === 'loading'}
            onClick={() => read(true)}
          >
            {t('common.refresh')}
          </Button>
        }
      />
      <div className="flex flex-col gap-3">
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

        {load.status === 'loading' ? (
          <Card role="status" className="flex items-center justify-center gap-2 py-10 text-muted">
            <Loader2 aria-hidden size={18} className="animate-spin motion-reduce:animate-none" />
            <p>{t('memory.reading')}</p>
          </Card>
        ) : null}

        {load.status === 'error' ? (
          <Card role="alert" className="flex flex-col items-center gap-3 py-10 text-center">
            <TriangleAlert aria-hidden size={24} className="text-muted" />
            <p className="font-semibold">{t('memory.error')}</p>
            <p className="max-w-xs text-body text-muted">{load.message}</p>
            <Button onClick={() => read()}>{t('common.tryAgain')}</Button>
          </Card>
        ) : null}

        {state?.lists ? <ListsCard lists={state.lists} /> : null}
        {state && !state.lists ? (
          <Card role="status" className="text-body text-muted">
            {state.problem
              ? t('memory.listsFailedWithProblem', { problem: state.problem })
              : t('memory.listsFailed')}
          </Card>
        ) : null}

        {state ? (
          <ListGroup aria-label={t('memory.operations')}>
            {state.operations.map((operation) => {
              const text = operationText(operation.area);
              const available = operation.state === 'available';
              const needsAdmin = operation.state === 'needsAdministrator';
              const viaHelper = needsAdmin && elevation?.state === 'available';
              const runnable = available || viaHelper;
              // Why a locked administrator action cannot run: the helper's own reason when it is
              // known, the operation's reason otherwise.
              const lockedReason =
                needsAdmin && elevation?.state === 'unavailable'
                  ? elevationReasonText(elevation.reason)
                  : operation.reason
                    ? reasonText(operation.state, operation.reason)
                    : null;
              return (
                <ListRow
                  key={operation.area}
                  title={text.title}
                  badge={
                    available ? null : viaHelper ? (
                      <Badge>
                        <ShieldCheck aria-hidden size={12} />
                        {t('memory.admin.badge')}
                      </Badge>
                    ) : (
                      <Badge>
                        <ShieldAlert aria-hidden size={12} />
                        {needsAdmin ? t('common.needsAdmin') : t('common.notSupported')}
                      </Badge>
                    )
                  }
                  description={
                    <>
                      <span className="block">{text.description}</span>
                      {viaHelper ? (
                        <span className="mt-0.5 block text-muted">{t('memory.runAdmin.hint')}</span>
                      ) : lockedReason ? (
                        <span className="mt-0.5 block font-semibold text-fg">{lockedReason}</span>
                      ) : null}
                    </>
                  }
                  trailing={
                    <Button
                      size="sm"
                      aria-label={
                        viaHelper
                          ? t('memory.runAdmin.label', { title: text.title })
                          : t('memory.run.label', { title: text.title })
                      }
                      icon={viaHelper ? <ShieldCheck aria-hidden size={16} /> : undefined}
                      loading={running === operation.area}
                      disabled={!runnable || (running !== null && running !== operation.area)}
                      onClick={() => void run(operation.area, viaHelper)}
                    >
                      {running === operation.area
                        ? t('memory.running')
                        : viaHelper
                          ? t('memory.runAdmin')
                          : t('memory.run')}
                    </Button>
                  }
                />
              );
            })}
          </ListGroup>
        ) : null}

        {report ? <ReportCard report={report} /> : null}
        <GuidanceCard />
      </div>
    </>
  );
}
