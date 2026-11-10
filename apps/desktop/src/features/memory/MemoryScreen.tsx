import { CircleCheck, Loader2, RefreshCw, ShieldAlert, TriangleAlert } from 'lucide-react';
import { useCallback, useEffect, useRef, useState } from 'react';
import {
  cleanMemory,
  getMemoryState,
  type MemoryAreaView,
  type MemoryCleanResult,
  type MemoryListsView,
  type MemoryState,
} from '../../shared/ipc';
import {
  describeArea,
  describeError,
  formatChange,
  listRows,
  operationText,
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
  const summary = listRows.map((row) => `${row.label} ${formatBytes(lists[row.key])}`).join(', ');
  return (
    <Card as="section" aria-label="Memory by list" className="flex flex-col gap-3">
      <p className="text-body">
        <span className="font-semibold">{formatBytes(lists.totalBytes)}</span> installed
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
                <dt className="font-semibold">{row.label}</dt>
                <dd className="tabular-nums">{formatBytes(lists[row.key])}</dd>
              </div>
              <p className="text-caption text-muted">{row.hint}</p>
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
    <Card as="section" aria-label="Result of the last run" className="flex flex-col gap-3">
      <h2 className="text-subtitle font-semibold">Result of the last run</h2>
      {before && after ? (
        <table className="w-full text-body">
          <thead>
            <tr className="text-left text-caption text-muted">
              <th scope="col" className="font-semibold">
                List
              </th>
              <th scope="col" className="text-right font-semibold">
                Before
              </th>
              <th scope="col" className="text-right font-semibold">
                After
              </th>
              <th scope="col" className="text-right font-semibold">
                Change
              </th>
            </tr>
          </thead>
          <tbody>
            {listRows.map((row) => (
              <tr key={row.key}>
                <th scope="row" className="py-0.5 text-left font-normal">
                  {row.label}
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
        <p className="text-body text-muted">Memory by list could not be read for this run.</p>
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
              <span className="font-semibold">{operationText[result.area].title}.</span>{' '}
              {describeArea(result)}
            </span>
          </li>
        ))}
      </ul>
      <p className="text-caption text-muted">
        This cannot be undone, and there is nothing to restore: Windows reloads what it needs.
        {report.recorded
          ? ' It is listed on the Undo page as a change that cannot be undone.'
          : ' It could not be written to the activity history.'}
      </p>
    </Card>
  );
}

function GuidanceCard() {
  return (
    <Card as="section" aria-label="When cleaning helps" className="flex flex-col gap-2">
      <h2 className="text-subtitle font-semibold">When cleaning helps, and when it does not</h2>
      <p className="text-body">
        <span className="font-semibold">It helps</span> right after a game, an editor or an AI model
        that held a lot of memory has closed and something else needs a large amount at once, or
        when you want a clean starting point to measure from.
      </p>
      <p className="text-body">
        <span className="font-semibold">It does not help</span> just because the standby number is
        large. Standby memory is a cache: Windows gives it back by itself the moment a program asks
        for memory, so it is not wasted. Emptying it makes the next things you open load from disk,
        which is slower. Cleaning also does not speed up a PC that has too little memory for what it
        runs, and it does not fix a program that keeps growing.
      </p>
    </Card>
  );
}

/**
 * The Memory screen: how memory is used by list (in use, standby, modified, free), one button
 * per cleaning operation and a before and after report for each run. Operations that need
 * administrator rights are listed with the reason and cannot be started. All rules live in the
 * Rust core: this renders what the commands report.
 */
export function MemoryScreen() {
  const [load, setLoad] = useState<Load>({ status: 'loading' });
  const [running, setRunning] = useState<MemoryAreaView | null>(null);
  const [report, setReport] = useState<MemoryCleanResult | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
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

  const run = async (area: MemoryAreaView) => {
    setRunning(area);
    setNotice(null);
    try {
      const result = await cleanMemory([area]);
      if (alive.current) {
        setReport(result);
        // Show what is true now, not the numbers from before the run.
        read(true);
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
        title="Memory"
        description="See how memory is used and release what Windows can spare. Each operation runs on its own, with a before and after."
        actions={
          <Button
            size="sm"
            icon={<RefreshCw aria-hidden size={16} />}
            disabled={running !== null || load.status === 'loading'}
            onClick={() => read(true)}
          >
            Refresh
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
              Dismiss
            </button>
          </div>
        ) : null}

        {load.status === 'loading' ? (
          <Card role="status" className="flex items-center justify-center gap-2 py-10 text-muted">
            <Loader2 aria-hidden size={18} className="animate-spin motion-reduce:animate-none" />
            <p>Reading memory...</p>
          </Card>
        ) : null}

        {load.status === 'error' ? (
          <Card role="alert" className="flex flex-col items-center gap-3 py-10 text-center">
            <TriangleAlert aria-hidden size={24} className="text-muted" />
            <p className="font-semibold">Memory could not be read</p>
            <p className="max-w-xs text-body text-muted">{load.message}</p>
            <Button onClick={() => read()}>Try again</Button>
          </Card>
        ) : null}

        {state?.lists ? <ListsCard lists={state.lists} /> : null}
        {state && !state.lists ? (
          <Card role="status" className="text-body text-muted">
            Memory by list could not be read{state.problem ? `: ${state.problem}` : '.'}
          </Card>
        ) : null}

        {state ? (
          <ListGroup aria-label="Cleaning operations">
            {state.operations.map((operation) => {
              const text = operationText[operation.area];
              const available = operation.state === 'available';
              return (
                <ListRow
                  key={operation.area}
                  title={text.title}
                  badge={
                    available ? null : (
                      <Badge>
                        <ShieldAlert aria-hidden size={12} />
                        {operation.state === 'needsAdministrator'
                          ? 'Needs administrator rights'
                          : 'Not supported'}
                      </Badge>
                    )
                  }
                  description={
                    <>
                      <span className="block">{text.description}</span>
                      {operation.reason ? (
                        <span className="mt-0.5 block font-semibold text-fg">
                          {operation.reason}
                        </span>
                      ) : null}
                    </>
                  }
                  trailing={
                    <Button
                      size="sm"
                      aria-label={`Run: ${text.title}`}
                      loading={running === operation.area}
                      disabled={!available || (running !== null && running !== operation.area)}
                      onClick={() => void run(operation.area)}
                    >
                      {running === operation.area ? 'Running...' : 'Run'}
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
