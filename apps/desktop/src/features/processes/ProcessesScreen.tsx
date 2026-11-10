import { Loader2, TriangleAlert } from 'lucide-react';
import { useCallback, useDeferredValue, useEffect, useMemo, useRef, useState } from 'react';
import { endApp, type ProcessGroupView } from '../../shared/ipc';
import { describeError } from '../../shared/text/memory';
import { countOf } from '../../shared/text/size';
import {
  Button,
  Card,
  ListGroup,
  PageHeader,
  SearchField,
  SegmentedControl,
  useToast,
} from '../../shared/ui';
import { EndDialog } from './EndDialog';
import { ProcessRow } from './ProcessRow';
import { endSummary, type SortKey, visibleGroups } from './processes';
import { useProcessList } from './useProcessList';

const sortOptions = [
  { value: 'memory', label: 'Memory' },
  { value: 'cpu', label: 'CPU' },
] as const;

function Notice({ message, onDismiss }: { message: string; onDismiss?: () => void }) {
  return (
    <div
      role="alert"
      className="flex items-start gap-3 rounded-md bg-caution p-3 text-body text-caution-fg"
    >
      <TriangleAlert aria-hidden size={18} className="mt-0.5 shrink-0" />
      <p className="min-w-0 flex-1">{message}</p>
      {onDismiss ? (
        <button
          type="button"
          className="shrink-0 rounded-sm px-2 py-1 font-semibold underline hover:no-underline"
          onClick={onDismiss}
        >
          Dismiss
        </button>
      ) : null}
    </div>
  );
}

/**
 * The Processes screen: what uses memory and CPU right now, grouped by app, refreshed about every
 * two seconds while the screen is open. An app can be ended after a confirmation. What may be
 * ended is decided by the Rust core (Windows-critical apps and Volt are never offered and are
 * refused there too): this renders what the commands report.
 */
export function ProcessesScreen() {
  const toast = useToast();
  const { load, refresh } = useProcessList();
  const [query, setQuery] = useState('');
  const [sort, setSort] = useState<SortKey>('memory');
  const [target, setTarget] = useState<ProcessGroupView | null>(null);
  const [ending, setEnding] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const alive = useRef(true);

  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  const groups = load.status === 'ready' ? load.list.groups : null;
  const searched = useDeferredValue(query);
  const shown = useMemo(
    () => (groups ? visibleGroups(groups, searched, sort) : []),
    [groups, searched, sort],
  );

  // A stable function, so a reading does not redraw rows whose numbers did not change.
  const askToEnd = useCallback((group: ProcessGroupView) => setTarget(group), []);

  const confirmEnd = async () => {
    if (!target) return;
    setEnding(true);
    setNotice(null);
    try {
      const summary = endSummary(await endApp(target.id));
      if (alive.current) {
        if (summary.problem) setNotice(summary.text);
        else toast.show({ message: summary.text });
      }
    } catch (error) {
      if (alive.current) setNotice(describeError(error));
    } finally {
      if (alive.current) {
        setEnding(false);
        setTarget(null);
        refresh();
      }
    }
  };

  const total = groups?.length ?? 0;
  return (
    <>
      <PageHeader
        title="Processes"
        description="What is using memory and CPU right now, grouped by app. Updates about every two seconds."
      />
      <div className="flex flex-col gap-3">
        {notice ? <Notice message={notice} onDismiss={() => setNotice(null)} /> : null}
        {load.status === 'ready' && load.problem ? (
          <Notice message={`The list could not be updated: ${load.problem}`} />
        ) : null}

        {load.status === 'loading' ? (
          <Card role="status" className="flex items-center justify-center gap-2 py-10 text-muted">
            <Loader2 aria-hidden size={18} className="animate-spin motion-reduce:animate-none" />
            <p>Reading processes...</p>
          </Card>
        ) : null}

        {load.status === 'error' ? (
          <Card role="alert" className="flex flex-col items-center gap-3 py-10 text-center">
            <TriangleAlert aria-hidden size={24} className="text-muted" />
            <p className="font-semibold">Processes could not be read</p>
            <p className="max-w-xs text-body text-muted">{load.message}</p>
            <Button onClick={refresh}>Try again</Button>
          </Card>
        ) : null}

        {load.status === 'ready' ? (
          <>
            <div className="flex flex-wrap items-center gap-3">
              <SearchField
                label="Search processes"
                placeholder="Search by name"
                value={query}
                onValueChange={setQuery}
                className="min-w-40 flex-1"
              />
              <SegmentedControl
                label="Sort by"
                options={sortOptions}
                value={sort}
                onValueChange={setSort}
              />
            </div>
            <p className="text-caption text-muted" aria-live="off">
              {searched.trim()
                ? `${shown.length} of ${countOf(total, 'app')} match`
                : `${countOf(total, 'app')}, ${countOf(load.list.processCount, 'process', 'processes')}`}
            </p>

            {shown.length > 0 ? (
              <div>
                <div
                  aria-hidden
                  className="flex items-center gap-3 px-4 pb-1 text-caption font-semibold text-muted"
                >
                  <span className="flex-1">App</span>
                  <span className="w-16 text-right">Memory</span>
                  <span className="w-12 text-right">CPU</span>
                  <span className="w-20" />
                </div>
                <ListGroup aria-label="Apps">
                  {shown.map((group) => (
                    <ProcessRow key={group.id} group={group} onEnd={askToEnd} />
                  ))}
                </ListGroup>
              </div>
            ) : (
              <Card role="status" className="py-8 text-center text-body text-muted">
                {searched.trim() ? `No app matches "${searched.trim()}".` : 'No processes found.'}
              </Card>
            )}
            <p className="text-caption text-muted">
              Memory is the private working set, the figure Task Manager shows. CPU is the share of
              all processors used since the last update. GPU use per app is not shown.
            </p>
          </>
        ) : null}
      </div>
      <EndDialog
        group={target}
        ending={ending}
        onCancel={() => setTarget(null)}
        onConfirm={() => void confirmEnd()}
      />
    </>
  );
}
