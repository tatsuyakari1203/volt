import { Loader2, Power, SearchX, ShieldAlert, TriangleAlert } from 'lucide-react';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { t, tn } from '../../shared/i18n';
import {
  listStartupEntries,
  type ScheduledTaskView,
  type ServiceView,
  type StartupEntryView,
  type StartupProblemView,
  type StartupProtectionView,
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
  approvalText,
  describeChange,
  describeChangeFailure,
  describeError,
  describeProblems,
  describeTasksProblem,
  isLocked,
  lockText,
  shortenCommand,
  sourceName,
} from './messages';

type Load =
  | { status: 'loading' }
  | { status: 'error'; message: string }
  | {
      status: 'ready';
      entries: StartupEntryView[];
      once: StartupEntryView[];
      problems: StartupProblemView[];
      tasks: ScheduledTaskView[];
      tasksProblem: string | null;
      services: ServiceView[];
      servicesProblem: string | null;
    };

/** What an entry is doing right now, while a command runs. */
type Busy = 'turning-on' | 'turning-off';

function publisherLine(item: { publisher: string | null }): string {
  return item.publisher
    ? t('startup.publisher', { name: item.publisher })
    : t('startup.publisher.unknown');
}

function summaryLine(entries: readonly StartupEntryView[]): string {
  const on = entries.filter((entry) => entry.enabled).length;
  return t('startup.summary', { entries: tn('count.entry', entries.length), on });
}

function matches(needle: string, ...parts: ReadonlyArray<string | null>): boolean {
  return parts
    .map((part) => part ?? '')
    .join(' ')
    .toLowerCase()
    .includes(needle);
}

function CommandText({ name, command }: { name: string; command: string }) {
  const [open, setOpen] = useState(false);
  const shortened = shortenCommand(command);
  // A command that already fits has nothing more to show.
  if (shortened === command.trim()) {
    return <code className="mt-0.5 block font-mono wrap-anywhere">{command}</code>;
  }
  return (
    <button
      type="button"
      aria-expanded={open}
      aria-label={t('startup.command.label', {
        name,
        state: open ? t('startup.command.showLess') : t('startup.command.showFull'),
      })}
      title={command}
      onClick={() => setOpen((value) => !value)}
      className="mt-0.5 block w-full rounded-sm text-left hover:bg-hover"
    >
      <code className="block font-mono wrap-anywhere">{open ? command : shortened}</code>
    </button>
  );
}

/** The badge that marks an item Windows publishes or needs; nothing for any other. */
function ProtectionBadge({ protection }: { protection: StartupProtectionView }) {
  if (protection === 'none') return null;
  return (
    <Badge>
      <ShieldAlert aria-hidden size={12} />
      {protection === 'critical'
        ? t('startup.protection.critical')
        : t('startup.protection.microsoft')}
    </Badge>
  );
}

interface EntryListProps {
  label: string;
  entries: readonly StartupEntryView[];
  unlocked: boolean;
  busy: Readonly<Record<string, Busy>>;
  onChange: (entry: StartupEntryView, wanted: boolean) => void;
}

/** Rows of switchable entries: what they are, who published them, why a switch is locked. */
function EntryList({ label, entries, unlocked, busy, onChange }: EntryListProps) {
  return (
    <ListGroup aria-label={label}>
      {entries.map((entry) => {
        const working = busy[entry.id];
        const lock = lockText(entry.lock, unlocked);
        return (
          <ListRow
            key={entry.id}
            title={entry.name}
            badge={
              <>
                <Badge>{sourceName(entry.source, entry.sourceLabel)}</Badge>
                <ProtectionBadge protection={entry.protection} />
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
                <CommandText name={entry.name} command={entry.command} />
                <span className="mt-0.5 block">{approvalText(entry.approval, entry.enabled)}</span>
                {lock ? <span className="mt-0.5 block">{lock}</span> : null}
                {working ? (
                  <span className="mt-0.5 block font-semibold text-fg">
                    {working === 'turning-on' ? t('startup.turningOn') : t('startup.turningOff')}
                  </span>
                ) : null}
              </>
            }
            trailing={
              <Switch
                aria-label={entry.name}
                aria-busy={working !== undefined}
                checked={entry.enabled}
                disabled={working !== undefined || isLocked(entry, unlocked)}
                onCheckedChange={(value) => onChange(entry, value)}
              />
            }
          />
        );
      })}
    </ListGroup>
  );
}

/**
 * The Startup screen: what starts with Windows, with a switch to turn each entry on or off the way
 * Windows does (it keeps the entry and records that it is off). Every change goes through the undo
 * journal and can be undone on the Undo page. Entries that apply to every user, services and
 * scheduled tasks are listed but cannot be changed yet; items Microsoft publishes stay locked
 * until the user allows changes to Windows items, and items Windows needs never unlock. All rules
 * live in the Rust core: this renders what the commands report.
 */
export function StartupScreen() {
  const toast = useToast();
  const [load, setLoad] = useState<Load>({ status: 'loading' });
  const [query, setQuery] = useState('');
  const [busy, setBusy] = useState<Readonly<Record<string, Busy>>>({});
  const [notice, setNotice] = useState<string | null>(null);
  const [showWindows, setShowWindows] = useState(false);
  const [unlocked, setUnlocked] = useState(false);
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
            once: list.once,
            problems: list.problems,
            tasks: list.tasks,
            tasksProblem: list.tasksProblem,
            services: list.services,
            servicesProblem: list.servicesProblem,
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
    const patch = (list: StartupEntryView[]) =>
      list.map((entry) => (entry.id === id ? { ...entry, enabled } : entry));
    setLoad((current) =>
      current.status === 'ready'
        ? { ...current, entries: patch(current.entries), once: patch(current.once) }
        : current,
    );
  };

  const change = async (entry: StartupEntryView, wanted: boolean) => {
    setBusyFor(entry.id, wanted ? 'turning-on' : 'turning-off');
    setNotice(null);
    try {
      const result = await setStartupEntryEnabled(entry.id, wanted, unlocked);
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

  const needle = query.trim().toLowerCase();
  const entries = load.status === 'ready' ? load.entries : [];
  const once = load.status === 'ready' ? load.once : [];
  const tasks = load.status === 'ready' ? load.tasks : [];
  const services = load.status === 'ready' ? load.services : [];
  const problems = load.status === 'ready' ? describeProblems(load.problems) : null;
  const tasksProblem = load.status === 'ready' ? describeTasksProblem(load.tasksProblem) : null;
  const servicesProblem = load.status === 'ready' ? load.servicesProblem : null;

  const visible = useMemo(
    () =>
      needle
        ? entries.filter((entry) =>
            matches(needle, entry.name, entry.publisher, entry.command, entry.sourceLabel),
          )
        : entries,
    [entries, needle],
  );
  const visibleOnce = useMemo(
    () =>
      needle
        ? once.filter((entry) =>
            matches(needle, entry.name, entry.publisher, entry.command, entry.sourceLabel),
          )
        : once,
    [once, needle],
  );
  // Windows' own tasks and services are many and rarely what a person is looking for: they are
  // hidden until the user asks to see them.
  const shownTasks = tasks.filter((task) => showWindows || task.protection === 'none');
  const shownServices = services.filter((service) => showWindows || service.protection === 'none');
  const hiddenCount = tasks.length - shownTasks.length + (services.length - shownServices.length);
  const visibleTasks = needle
    ? shownTasks.filter((task) => matches(needle, task.name, task.publisher, task.command))
    : shownTasks;
  const visibleServices = needle
    ? shownServices.filter((service) =>
        matches(needle, service.name, service.displayName, service.publisher, service.command),
      )
    : shownServices;
  const hasProtected =
    entries.some((entry) => entry.protection !== 'none') ||
    once.some((entry) => entry.protection !== 'none') ||
    tasks.some((task) => task.protection !== 'none') ||
    services.some((service) => service.protection !== 'none');
  const nothing =
    entries.length === 0 && once.length === 0 && tasks.length === 0 && services.length === 0;
  const listed = entries.length + once.length + tasks.length + services.length;

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

        {load.status === 'ready' && nothing && !servicesProblem && !tasksProblem ? (
          <Card className="flex flex-col items-center gap-2 py-10 text-center">
            <Power aria-hidden size={24} className="text-muted" />
            <p className="font-semibold">{t('startup.empty.title')}</p>
            <p className="max-w-xs text-body text-muted">{t('startup.empty.description')}</p>
          </Card>
        ) : null}

        {load.status === 'ready' && hasProtected ? (
          <section aria-labelledby="startup-windows-title" className="flex flex-col gap-2">
            <h2 id="startup-windows-title" className="text-body font-semibold">
              {t('startup.show.title')}
            </h2>
            <ListGroup aria-label={t('startup.show.title')}>
              <ListRow
                title={t('startup.show.label')}
                description={
                  <>
                    <span className="block">{t('startup.show.hint')}</span>
                    {!showWindows && hiddenCount > 0 ? (
                      <span className="mt-0.5 block">
                        {t('startup.show.hidden', { count: hiddenCount })}
                      </span>
                    ) : null}
                  </>
                }
                trailing={
                  <Switch
                    aria-label={t('startup.show.label')}
                    checked={showWindows}
                    onCheckedChange={(value) => {
                      setShowWindows(value);
                      if (!value) setUnlocked(false);
                    }}
                  />
                }
              />
              <ListRow
                title={t('startup.unlock.label')}
                description={t('startup.unlock.hint')}
                trailing={
                  <Switch
                    aria-label={t('startup.unlock.label')}
                    checked={unlocked}
                    disabled={!showWindows}
                    onCheckedChange={setUnlocked}
                  />
                }
              />
            </ListGroup>
          </section>
        ) : null}

        {load.status === 'ready' && listed > 0 ? (
          <SearchField
            label={t('startup.search')}
            placeholder={t('startup.search')}
            value={query}
            onValueChange={setQuery}
          />
        ) : null}

        {load.status === 'ready' && entries.length > 0 ? (
          <>
            <p className="text-caption text-muted">{summaryLine(entries)}</p>
            {visible.length > 0 ? (
              <EntryList
                label={t('startup.list.label')}
                entries={visible}
                unlocked={unlocked}
                busy={busy}
                onChange={(entry, wanted) => void change(entry, wanted)}
              />
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

        {load.status === 'ready' && once.length > 0 ? (
          <section aria-labelledby="startup-once-title" className="flex flex-col gap-2">
            <h2 id="startup-once-title" className="text-body font-semibold">
              {t('startup.once.title')}
            </h2>
            <p className="text-caption text-muted">{t('startup.once.note')}</p>
            {visibleOnce.length > 0 ? (
              <EntryList
                label={t('startup.once.list.label')}
                entries={visibleOnce}
                unlocked={unlocked}
                busy={busy}
                onChange={(entry, wanted) => void change(entry, wanted)}
              />
            ) : (
              <p className="text-caption text-muted">
                {t('startup.noMatch.title', { query: query.trim() })}
              </p>
            )}
          </section>
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
                        {task.protection === 'critical' ? (
                          <ProtectionBadge protection="critical" />
                        ) : null}
                      </>
                    }
                    description={
                      <>
                        <span className="block">{publisherLine(task)}</span>
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
                        <span className="mt-0.5 block">{lockText(task.lock, unlocked)}</span>
                      </>
                    }
                  />
                ))}
              </ListGroup>
            ) : tasks.length > 0 && needle ? (
              <p className="text-caption text-muted">
                {t('startup.tasks.noMatch', { query: query.trim() })}
              </p>
            ) : null}
          </section>
        ) : null}

        {load.status === 'ready' && (services.length > 0 || servicesProblem) ? (
          <section aria-labelledby="startup-services-title" className="flex flex-col gap-2">
            <h2 id="startup-services-title" className="text-body font-semibold">
              {t('startup.services.title')}
            </h2>
            <p className="text-caption text-muted">{t('startup.services.note')}</p>
            {servicesProblem ? (
              <p role="status" className="text-caption text-muted">
                {t('startup.services.problem', { reason: servicesProblem })}
              </p>
            ) : null}
            {visibleServices.length > 0 ? (
              <ListGroup aria-label={t('startup.services.list.label')}>
                {visibleServices.map((service) => (
                  <ListRow
                    key={service.name}
                    title={service.displayName}
                    badge={
                      <>
                        <Badge>
                          {service.delayed
                            ? t('startup.services.delayed')
                            : t('startup.services.automatic')}
                        </Badge>
                        <ProtectionBadge protection={service.protection} />
                      </>
                    }
                    description={
                      <>
                        <span className="block">{publisherLine(service)}</span>
                        {service.command ? (
                          <code
                            className="mt-0.5 block font-mono wrap-anywhere"
                            title={service.command}
                          >
                            {shortenCommand(service.command)}
                          </code>
                        ) : null}
                        {service.account ? (
                          <span className="mt-0.5 block">
                            {t('startup.services.account', { account: service.account })}
                          </span>
                        ) : null}
                        <span className="mt-0.5 block">{lockText(service.lock, unlocked)}</span>
                      </>
                    }
                  />
                ))}
              </ListGroup>
            ) : services.length > 0 && needle ? (
              <p className="text-caption text-muted">
                {t('startup.services.noMatch', { query: query.trim() })}
              </p>
            ) : null}
          </section>
        ) : null}
      </div>
    </>
  );
}
