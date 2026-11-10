import { Loader2, Power, SearchX, ShieldAlert, TriangleAlert } from 'lucide-react';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { t, tn } from '../../shared/i18n';
import {
  listStartupEntries,
  type ScheduledTaskView,
  type StartupEntryView,
  type StartupProblemView,
  setStartupEntryEnabled,
} from '../../shared/ipc';
import {
  Badge,
  Button,
  Card,
  ListGroup,
  ListRow,
  PageHeader,
  SearchField,
  Switch,
  useToast,
} from '../../shared/ui';
import {
  describeChange,
  describeChangeFailure,
  describeError,
  describeProblems,
  describeTasksProblem,
  shortenCommand,
  sourceName,
} from './messages';

type Load =
  | { status: 'loading' }
  | { status: 'error'; message: string }
  | {
      status: 'ready';
      entries: StartupEntryView[];
      problems: StartupProblemView[];
      tasks: ScheduledTaskView[];
      tasksProblem: string | null;
    };

/** What an entry is doing right now, while a command runs. */
type Busy = 'turning-on' | 'turning-off';

function publisherLine(entry: StartupEntryView): string {
  return entry.publisher
    ? t('startup.publisher', { name: entry.publisher })
    : t('startup.publisher.unknown');
}

function summaryLine(entries: readonly StartupEntryView[]): string {
  const on = entries.filter((entry) => entry.enabled).length;
  return t('startup.summary', { entries: tn('count.entry', entries.length), on });
}

function CommandText({ entry }: { entry: StartupEntryView }) {
  const [open, setOpen] = useState(false);
  const shortened = shortenCommand(entry.command);
  // A command that already fits has nothing more to show.
  if (shortened === entry.command.trim()) {
    return <code className="mt-0.5 block font-mono wrap-anywhere">{entry.command}</code>;
  }
  return (
    <button
      type="button"
      aria-expanded={open}
      aria-label={t('startup.command.label', {
        name: entry.name,
        state: open ? t('startup.command.showLess') : t('startup.command.showFull'),
      })}
      title={entry.command}
      onClick={() => setOpen((value) => !value)}
      className="mt-0.5 block w-full rounded-sm text-left hover:bg-hover"
    >
      <code className="block font-mono wrap-anywhere">{open ? entry.command : shortened}</code>
    </button>
  );
}

/**
 * The Startup screen: what starts with Windows, with a switch to turn each entry on or off the way
 * Windows does (it keeps the entry and records that it is off). Every change goes through the undo
 * journal and can be undone on the Undo page. Entries that apply to every user are listed but
 * cannot be changed yet, because that needs administrator rights. All rules live in the Rust core:
 * this renders what the commands report.
 */
export function StartupScreen() {
  const toast = useToast();
  const [load, setLoad] = useState<Load>({ status: 'loading' });
  const [query, setQuery] = useState('');
  const [busy, setBusy] = useState<Readonly<Record<string, Busy>>>({});
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
    listStartupEntries()
      .then((list) => {
        if (alive.current) {
          setLoad({
            status: 'ready',
            entries: list.entries,
            problems: list.problems,
            tasks: list.tasks,
            tasksProblem: list.tasksProblem,
          });
        }
      })
      .catch((error: unknown) => {
        if (alive.current) setLoad({ status: 'error', message: describeError(error) });
      });
  }, []);

  useEffect(() => read(), [read]);

  const setBusyFor = (id: string, value: Busy | undefined) => {
    if (!alive.current) return;
    setBusy((current) => {
      const next = { ...current };
      if (value) next[id] = value;
      else delete next[id];
      return next;
    });
  };

  const patchEnabled = (id: string, enabled: boolean) => {
    setLoad((current) =>
      current.status === 'ready'
        ? {
            ...current,
            entries: current.entries.map((entry) =>
              entry.id === id ? { ...entry, enabled } : entry,
            ),
          }
        : current,
    );
  };

  const change = async (entry: StartupEntryView, wanted: boolean) => {
    setBusyFor(entry.id, wanted ? 'turning-on' : 'turning-off');
    setNotice(null);
    try {
      const result = await setStartupEntryEnabled(entry.id, wanted);
      if (alive.current) {
        if (result.enabled === null) read(true);
        else patchEnabled(entry.id, result.enabled);
      }
      const outcome = describeChange(entry, wanted, result);
      if (outcome.tone === 'problem') {
        if (alive.current) setNotice(outcome.message);
      } else {
        toast.show({ message: outcome.message });
      }
    } catch (error) {
      if (alive.current) {
        setNotice(describeChangeFailure(entry, wanted, error));
        // The entry may be gone or changed outside Volt: show what is true now.
        read(true);
      }
    } finally {
      setBusyFor(entry.id, undefined);
    }
  };

  const entries = load.status === 'ready' ? load.entries : [];
  const problems = load.status === 'ready' ? describeProblems(load.problems) : null;
  const visible = useMemo(() => {
    const needle = query.trim().toLowerCase();
    if (!needle) return entries;
    return entries.filter((entry) =>
      `${entry.name} ${entry.publisher ?? ''} ${entry.command} ${entry.sourceLabel}`
        .toLowerCase()
        .includes(needle),
    );
  }, [entries, query]);
  const tasks = load.status === 'ready' ? load.tasks : [];
  const tasksProblem = load.status === 'ready' ? describeTasksProblem(load.tasksProblem) : null;
  const visibleTasks = useMemo(() => {
    const needle = query.trim().toLowerCase();
    if (!needle) return tasks;
    return tasks.filter((task) =>
      `${task.name} ${task.publisher ?? ''} ${task.command}`.toLowerCase().includes(needle),
    );
  }, [tasks, query]);

  return (
    <>
      <PageHeader title={t('nav.startup')} description={t('startup.description')} />
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

        {problems ? (
          <div
            role="status"
            className="flex items-start gap-3 rounded-md bg-caution p-3 text-body text-caution-fg"
          >
            <TriangleAlert aria-hidden size={18} className="mt-0.5 shrink-0" />
            <p className="min-w-0 flex-1">{problems}</p>
          </div>
        ) : null}

        {load.status === 'loading' ? (
          <Card role="status" className="flex items-center justify-center gap-2 py-10 text-muted">
            <Loader2 aria-hidden size={18} className="animate-spin motion-reduce:animate-none" />
            <p>{t('startup.loading')}</p>
          </Card>
        ) : null}

        {load.status === 'error' ? (
          <Card role="alert" className="flex flex-col items-center gap-3 py-10 text-center">
            <TriangleAlert aria-hidden size={24} className="text-muted" />
            <p className="font-semibold">{t('startup.error')}</p>
            <p className="max-w-xs text-body text-muted">{load.message}</p>
            <Button onClick={() => read()}>{t('common.tryAgain')}</Button>
          </Card>
        ) : null}

        {load.status === 'ready' && entries.length === 0 && tasks.length === 0 ? (
          <Card className="flex flex-col items-center gap-2 py-10 text-center">
            <Power aria-hidden size={24} className="text-muted" />
            <p className="font-semibold">{t('startup.empty.title')}</p>
            <p className="max-w-xs text-body text-muted">{t('startup.empty.description')}</p>
          </Card>
        ) : null}

        {load.status === 'ready' && entries.length > 0 ? (
          <>
            <SearchField
              label={t('startup.search')}
              placeholder={t('startup.search')}
              value={query}
              onValueChange={setQuery}
            />
            <p className="text-caption text-muted">{summaryLine(entries)}</p>
            {visible.length > 0 ? (
              <ListGroup aria-label={t('startup.list.label')}>
                {visible.map((entry) => {
                  const working = busy[entry.id];
                  return (
                    <ListRow
                      key={entry.id}
                      title={entry.name}
                      badge={
                        <>
                          <Badge>{sourceName(entry.source, entry.sourceLabel)}</Badge>
                          {entry.needsAdmin ? (
                            <Badge>
                              <ShieldAlert aria-hidden size={12} />
                              {t('common.needsAdmin')}
                            </Badge>
                          ) : null}
                        </>
                      }
                      description={
                        <>
                          <span className="block">{publisherLine(entry)}</span>
                          <CommandText entry={entry} />
                          {working ? (
                            <span className="mt-0.5 block font-semibold text-fg">
                              {working === 'turning-on'
                                ? t('startup.turningOn')
                                : t('startup.turningOff')}
                            </span>
                          ) : null}
                        </>
                      }
                      trailing={
                        <Switch
                          aria-label={entry.name}
                          aria-busy={working !== undefined}
                          checked={entry.enabled}
                          disabled={working !== undefined || entry.needsAdmin}
                          onCheckedChange={(value) => void change(entry, value)}
                        />
                      }
                    />
                  );
                })}
              </ListGroup>
            ) : (
              <Card className="flex flex-col items-center gap-2 py-10 text-center">
                <SearchX aria-hidden size={24} className="text-muted" />
                <p className="font-semibold">
                  {t('startup.noMatch.title', { query: query.trim() })}
                </p>
                <p className="text-body text-muted">{t('startup.noMatch.hint')}</p>
              </Card>
            )}
          </>
        ) : null}

        {load.status === 'ready' && (tasks.length > 0 || tasksProblem) ? (
          <section aria-labelledby="startup-tasks-title" className="flex flex-col gap-2">
            <h2 id="startup-tasks-title" className="text-body font-semibold">
              {t('startup.tasks.title')}
            </h2>
            <p className="text-caption text-muted">{t('startup.tasks.note')}</p>
            {tasksProblem ? (
              <p role="status" className="text-caption text-muted">
                {tasksProblem}
              </p>
            ) : null}
            {visibleTasks.length > 0 ? (
              <ListGroup aria-label={t('startup.tasks.list.label')}>
                {visibleTasks.map((task) => (
                  <ListRow
                    key={`${task.folder}/${task.name}`}
                    title={task.name}
                    badge={
                      <>
                        <Badge>
                          {task.microsoft
                            ? t('startup.tasks.microsoft')
                            : t('startup.tasks.thirdParty')}
                        </Badge>
                        <Badge>
                          {task.enabled ? t('startup.tasks.enabled') : t('startup.tasks.disabled')}
                        </Badge>
                      </>
                    }
                    description={
                      <>
                        <span className="block">
                          {task.publisher
                            ? t('startup.publisher', { name: task.publisher })
                            : t('startup.publisher.unknown')}
                        </span>
                        {task.command ? (
                          <code
                            className="mt-0.5 block font-mono wrap-anywhere"
                            title={task.command}
                          >
                            {shortenCommand(task.command)}
                          </code>
                        ) : (
                          <span className="mt-0.5 block">{t('startup.tasks.noProgram')}</span>
                        )}
                      </>
                    }
                  />
                ))}
              </ListGroup>
            ) : tasks.length > 0 ? (
              <p className="text-caption text-muted">
                {t('startup.tasks.noMatch', { query: query.trim() })}
              </p>
            ) : null}
          </section>
        ) : null}
      </div>
    </>
  );
}
