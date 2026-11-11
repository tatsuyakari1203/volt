import { AppWindow, Loader2, SearchX, TriangleAlert } from 'lucide-react';
import { useCallback, useDeferredValue, useEffect, useMemo, useRef, useState } from 'react';
import { t, tn } from '../../shared/i18n';
import { type AppListView, type AppView, listApps } from '../../shared/ipc';
import { describeError } from '../../shared/text/errors';
import {
  Badge,
  Button,
  Card,
  ListGroup,
  PageHeader,
  SearchField,
  SegmentedControl,
  Switch,
} from '../../shared/ui';
import {
  detailsText,
  hiddenCount,
  hiddenText,
  problemText,
  type SortKey,
  selectedApps,
  visibleApps,
} from './apps';
import { QueuePanel } from './QueuePanel';
import { forgetQueue, type PendingApp, readRememberedQueue } from './queueMemory';
import { UninstallDialog } from './UninstallDialog';
import { useUninstallQueue } from './useUninstallQueue';

type Load =
  | { status: 'loading' }
  | { status: 'error'; message: string }
  | { status: 'ready'; list: AppListView };

function sortOptions() {
  return [
    { value: 'name', label: t('apps.sort.name') },
    { value: 'size', label: t('apps.sort.size') },
    { value: 'date', label: t('apps.sort.date') },
  ] as const;
}

function AppRow({
  app,
  ticked,
  disabled,
  onToggle,
}: {
  app: AppView;
  ticked: boolean;
  disabled: boolean;
  onToggle: (id: string, on: boolean) => void;
}) {
  const details = detailsText(app);
  return (
    <li>
      <label className="flex items-start gap-3 px-4 py-3">
        <input
          type="checkbox"
          className="mt-1 size-4 shrink-0 accent-(--color-accent)"
          aria-label={t('apps.row.select', { name: app.name })}
          checked={ticked}
          disabled={disabled || !app.uninstallable}
          onChange={(event) => onToggle(app.id, event.target.checked)}
        />
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
            <span className="wrap-anywhere font-semibold">{app.name}</span>
            {app.source === 'currentUser' ? (
              <span title={t('apps.source.currentUser.detail')}>
                <Badge>{t('apps.source.currentUser')}</Badge>
              </span>
            ) : null}
            {app.source === 'packaged' ? <Badge>{t('apps.source.packaged')}</Badge> : null}
            {app.hidden ? <Badge>{hiddenText(app.hidden)}</Badge> : null}
          </div>
          <p className="wrap-anywhere text-caption text-muted">
            {app.publisher ?? t('apps.row.publisherUnknown')}
          </p>
          {details ? <p className="text-caption text-muted">{details}</p> : null}
          {app.uninstallable ? null : (
            <p className="text-caption text-muted">{t('apps.row.cannotRemove')}</p>
          )}
        </div>
      </label>
    </li>
  );
}

/**
 * The Apps screen: what is installed (desktop apps and the current user's Store apps) with size,
 * publisher and install date, searchable and sortable. Tick several apps and Volt runs each one's
 * own uninstaller in turn, after one confirmation. System components, updates, frameworks and
 * apps that cannot be removed are hidden until the switch is turned on. What is listed and what
 * may be started is decided by the Rust core; this renders what the commands report.
 */
export function AppsScreen() {
  const [load, setLoad] = useState<Load>({ status: 'loading' });
  const [query, setQuery] = useState('');
  const [sort, setSort] = useState<SortKey>('name');
  const [showSystem, setShowSystem] = useState(false);
  const [ticked, setTicked] = useState<ReadonlySet<string>>(new Set());
  const [confirming, setConfirming] = useState(false);
  const alive = useRef(true);

  const onListing = useCallback((list: AppListView) => setLoad({ status: 'ready', list }), []);
  const { queue, start, stop, stopWaiting, dismiss } = useUninstallQueue(onListing);
  // What an earlier queue left unfinished when its page or window was closed.
  const [left, setLeft] = useState<PendingApp[]>(readRememberedQueue);

  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  const read = useCallback(() => {
    setLoad({ status: 'loading' });
    listApps()
      .then((list) => {
        if (alive.current) setLoad({ status: 'ready', list });
      })
      .catch((error: unknown) => {
        if (alive.current) setLoad({ status: 'error', message: describeError(error) });
      });
  }, []);

  useEffect(() => read(), [read]);

  const apps = useMemo(() => (load.status === 'ready' ? load.list.apps : []), [load]);
  const searched = useDeferredValue(query);
  const shown = useMemo(
    () => visibleApps(apps, searched, sort, showSystem),
    [apps, searched, sort, showSystem],
  );
  const chosen = useMemo(() => selectedApps(apps, ticked, showSystem), [apps, ticked, showSystem]);
  const hidden = hiddenCount(apps);
  const running = queue?.running ?? false;

  const toggle = useCallback((id: string, on: boolean) => {
    setTicked((current) => {
      const next = new Set(current);
      if (on) next.add(id);
      else next.delete(id);
      return next;
    });
  }, []);

  const confirm = () => {
    setConfirming(false);
    setLeft([]);
    setTicked(new Set());
    void start(chosen, showSystem);
  };

  const problems = load.status === 'ready' ? load.list.problems : [];
  const notRemoved = left.filter((item) => apps.some((app) => app.id === item.id));
  const dismissLeft = () => {
    forgetQueue();
    setLeft([]);
  };

  return (
    <>
      <PageHeader title={t('nav.apps')} description={t('apps.description')} />
      <div className="flex flex-col gap-3">
        {problems.map((problem) => (
          <div
            key={problem.source}
            role="status"
            className="flex items-start gap-3 rounded-md bg-caution p-3 text-body text-caution-fg"
          >
            <TriangleAlert aria-hidden size={18} className="mt-0.5 shrink-0" />
            <p className="min-w-0 flex-1">
              {problemText(problem, (count) => tn('count.entry', count))}
            </p>
          </div>
        ))}

        {notRemoved.length > 0 ? (
          <div
            role="status"
            className="flex items-start gap-3 rounded-md bg-caution p-3 text-body text-caution-fg"
          >
            <TriangleAlert aria-hidden size={18} className="mt-0.5 shrink-0" />
            <p className="min-w-0 flex-1">
              {t('apps.interrupted', { apps: notRemoved.map((item) => item.name).join(', ') })}
            </p>
            <Button size="sm" onClick={dismissLeft}>
              {t('apps.interrupted.dismiss')}
            </Button>
          </div>
        ) : null}

        {load.status === 'loading' ? (
          <Card role="status" className="flex items-center justify-center gap-2 py-10 text-muted">
            <Loader2 aria-hidden size={18} className="animate-spin motion-reduce:animate-none" />
            <p>{t('apps.loading')}</p>
          </Card>
        ) : null}

        {load.status === 'error' ? (
          <Card role="alert" className="flex flex-col items-center gap-3 py-10 text-center">
            <TriangleAlert aria-hidden size={24} className="text-muted" />
            <p className="font-semibold">{t('apps.error')}</p>
            <p className="max-w-xs text-body text-muted">{load.message}</p>
            <Button onClick={read}>{t('common.tryAgain')}</Button>
          </Card>
        ) : null}

        {load.status === 'ready' ? (
          <>
            <SearchField
              label={t('apps.search.label')}
              placeholder={t('apps.search.placeholder')}
              value={query}
              onValueChange={setQuery}
            />
            <div className="flex flex-wrap items-center justify-between gap-3">
              <SegmentedControl
                label={t('apps.sort.label')}
                options={sortOptions()}
                value={sort}
                onValueChange={setSort}
              />
              <div className="flex items-center gap-2">
                <span id="apps-system-label" className="text-body">
                  {t('apps.system.label')}
                </span>
                <Switch
                  aria-labelledby="apps-system-label"
                  aria-describedby="apps-system-hint"
                  checked={showSystem}
                  onCheckedChange={setShowSystem}
                />
              </div>
            </div>
            <p id="apps-system-hint" className="text-caption text-muted">
              {t('apps.system.hint')}
            </p>
            <p className="text-caption text-muted" aria-live="polite">
              {hidden > 0 && !showSystem
                ? t('apps.summary.hidden', { apps: tn('count.app', shown.length), hidden })
                : t('apps.summary', { apps: tn('count.app', shown.length) })}
            </p>

            {shown.length > 0 ? (
              <ListGroup aria-label={t('apps.list.label')}>
                {shown.map((app) => (
                  <AppRow
                    key={app.id}
                    app={app}
                    ticked={ticked.has(app.id)}
                    disabled={queue !== null}
                    onToggle={toggle}
                  />
                ))}
              </ListGroup>
            ) : (
              <Card role="status" className="flex flex-col items-center gap-2 py-10 text-center">
                {searched.trim() ? (
                  <>
                    <SearchX aria-hidden size={24} className="text-muted" />
                    <p className="font-semibold">{t('apps.noMatch', { query: searched.trim() })}</p>
                  </>
                ) : (
                  <>
                    <AppWindow aria-hidden size={24} className="text-muted" />
                    <p className="font-semibold">{t('apps.empty.title')}</p>
                    <p className="max-w-xs text-body text-muted">{t('apps.empty.description')}</p>
                  </>
                )}
              </Card>
            )}

            {queue ? (
              <QueuePanel
                queue={queue}
                onStop={stop}
                onStopWaiting={stopWaiting}
                onDone={dismiss}
              />
            ) : (
              <Card
                as="section"
                aria-label={t('apps.selection.none')}
                className="sticky bottom-3 z-10 flex flex-col gap-3 shadow-pop"
              >
                <p aria-live="polite">
                  {chosen.length === 0
                    ? t('apps.selection.none')
                    : t('apps.selection.some', { apps: tn('count.app', chosen.length) })}
                </p>
                <p className="text-caption text-muted">{t('apps.noUndo')}</p>
                <div className="flex flex-wrap gap-2">
                  <Button
                    variant="primary"
                    disabled={chosen.length === 0 || running}
                    onClick={() => setConfirming(true)}
                  >
                    {tn('apps.uninstall', chosen.length)}
                  </Button>
                  <Button disabled={ticked.size === 0} onClick={() => setTicked(new Set())}>
                    {t('apps.clear')}
                  </Button>
                </div>
              </Card>
            )}
            <p className="text-caption text-muted">{t('apps.footnote')}</p>
          </>
        ) : null}
      </div>
      <UninstallDialog
        apps={confirming ? chosen : null}
        onCancel={() => setConfirming(false)}
        onConfirm={confirm}
      />
    </>
  );
}
