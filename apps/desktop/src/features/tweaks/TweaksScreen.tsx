import { Loader2, SearchX, SlidersHorizontal, TriangleAlert } from 'lucide-react';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { t, tn } from '../../shared/i18n';
import {
  applyTweak,
  getTweakPlan,
  listTweaks,
  resetTweak,
  revertTweak,
  type TweakCategory,
  type TweakInfo,
  type TweakStateView,
} from '../../shared/ipc';
import { getStorage } from '../../shared/theme/theme';
import {
  Button,
  Card,
  Dialog,
  ListGroup,
  PageHeader,
  SearchField,
  SegmentedControl,
  useToast,
} from '../../shared/ui';
import {
  CATEGORY_ORDER,
  categoryLabel,
  describeApply,
  describeError,
  describeFailure,
  describeReset,
  describeRevert,
  type Outcome,
  tweakDescription,
  tweakTitle,
} from './messages';
import { type Busy, type PlanLoad, StepList, TweakRow } from './TweakRow';

type Load =
  | { status: 'loading' }
  | { status: 'error'; message: string }
  | { status: 'ready'; tweaks: TweakInfo[] };

type Mode = 'simple' | 'advanced';
type CategoryFilter = 'all' | TweakCategory;

/** localStorage key of the remembered mode. */
export const MODE_STORAGE_KEY = 'volt.tweaks.mode';

function readMode(): Mode {
  try {
    return getStorage()?.getItem(MODE_STORAGE_KEY) === 'advanced' ? 'advanced' : 'simple';
  } catch {
    return 'simple';
  }
}

function writeMode(mode: Mode) {
  try {
    getStorage()?.setItem(MODE_STORAGE_KEY, mode);
  } catch {
    // The choice is only a convenience; the screen works without remembering it.
  }
}

/** A question waiting for the user: turn on something above Safe, or turn off what Volt did not. */
interface Confirmation {
  kind: 'apply' | 'reset';
  tweak: TweakInfo;
  plan: PlanLoad;
}

/**
 * The Tweaks screen. Simple mode (the default) lists only Safe tweaks as switches grouped by
 * category. Advanced mode lists every tweak with its risk, evidence, restart requirement and the
 * exact changes the engine would make. State is detected whenever the screen opens, so a tweak
 * that was changed outside Volt shows as it is. All rules live in the Rust core: this renders
 * what the commands report.
 */
export function TweaksScreen() {
  const toast = useToast();
  const [load, setLoad] = useState<Load>({ status: 'loading' });
  const [mode, setMode] = useState<Mode>(readMode);
  const [query, setQuery] = useState('');
  const [category, setCategory] = useState<CategoryFilter>('all');
  const [busy, setBusy] = useState<Readonly<Record<string, Busy>>>({});
  const [plans, setPlans] = useState<Readonly<Record<string, PlanLoad>>>({});
  const [notice, setNotice] = useState<string | null>(null);
  const [confirming, setConfirming] = useState<Confirmation | null>(null);
  const cancelRef = useRef<HTMLButtonElement>(null);
  const alive = useRef(true);

  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  const read = useCallback(() => {
    setLoad({ status: 'loading' });
    listTweaks()
      .then((tweaks) => {
        if (alive.current) setLoad({ status: 'ready', tweaks });
      })
      .catch((error: unknown) => {
        if (alive.current) setLoad({ status: 'error', message: describeError(error) });
      });
  }, []);

  useEffect(read, [read]);

  const patch = (id: string, change: Partial<TweakInfo>) => {
    setLoad((current) =>
      current.status === 'ready'
        ? {
            status: 'ready',
            tweaks: current.tweaks.map((tweak) =>
              tweak.id === id ? { ...tweak, ...change } : tweak,
            ),
          }
        : current,
    );
  };

  /** Records a new state; a change Volt made is Volt's to undo, one it reset is not. */
  const patchState = (id: string, state: TweakStateView, appliedByVolt: boolean) => {
    patch(id, { state, appliedByVolt });
    setPlans((current) => {
      if (!(id in current)) return current;
      const next = { ...current };
      delete next[id];
      return next;
    });
  };

  const setBusyFor = (id: string, value: Busy | undefined) => {
    if (!alive.current) return;
    setBusy((current) => {
      const next = { ...current };
      if (value) next[id] = value;
      else delete next[id];
      return next;
    });
  };

  const report = (outcome: Outcome, tweak: TweakInfo) => {
    if (outcome.tone === 'problem') {
      if (alive.current) setNotice(outcome.message);
      return;
    }
    toast.show({
      message: outcome.message,
      ...(outcome.undoable
        ? { actionLabel: t('common.undo'), onAction: () => void revert(tweak, true) }
        : {}),
    });
  };

  const apply = async (tweak: TweakInfo) => {
    setBusyFor(tweak.id, 'applying');
    setNotice(null);
    try {
      const result = await applyTweak(tweak.id);
      const changed = result.status === 'applied' || result.status === 'needsAttention';
      if (alive.current) patchState(tweak.id, result.state, changed || tweak.appliedByVolt);
      report(describeApply(tweak, result), tweak);
    } catch (error) {
      if (alive.current) setNotice(describeFailure('turn on', tweak, error));
    } finally {
      setBusyFor(tweak.id, undefined);
    }
  };

  const revert = async (tweak: TweakInfo, afterUndo: boolean) => {
    setBusyFor(tweak.id, 'undoing');
    setNotice(null);
    try {
      const result = await revertTweak(tweak.id);
      if (alive.current) patchState(tweak.id, result.state, false);
      report(describeRevert(tweak, result, afterUndo), tweak);
    } catch (error) {
      if (alive.current) setNotice(describeFailure('undo', tweak, error));
    } finally {
      setBusyFor(tweak.id, undefined);
    }
  };

  const reset = async (tweak: TweakInfo) => {
    setBusyFor(tweak.id, 'undoing');
    setNotice(null);
    try {
      const result = await resetTweak(tweak.id);
      if (alive.current) patchState(tweak.id, result.state, false);
      report(describeReset(tweak, result), tweak);
    } catch (error) {
      if (alive.current) setNotice(describeFailure('undo', tweak, error));
    } finally {
      setBusyFor(tweak.id, undefined);
    }
  };

  /** Reads what a tweak changes; `null` plan state is reported as an error line. */
  const readPlan = (tweak: TweakInfo, done: (plan: PlanLoad) => void) => {
    getTweakPlan(tweak.id)
      .then((plan) => {
        if (alive.current) done({ status: 'ready', plan });
      })
      .catch((error: unknown) => {
        if (alive.current) done({ status: 'error', message: describeError(error) });
      });
  };

  const showPlan = (tweak: TweakInfo, open: boolean) => {
    if (!open) {
      setPlans((current) => {
        const next = { ...current };
        delete next[tweak.id];
        return next;
      });
      return;
    }
    setPlans((current) => ({ ...current, [tweak.id]: { status: 'loading' } }));
    readPlan(tweak, (plan) => setPlans((current) => ({ ...current, [tweak.id]: plan })));
  };

  const confirm = (kind: Confirmation['kind'], tweak: TweakInfo) => {
    setConfirming({ kind, tweak, plan: { status: 'loading' } });
    readPlan(tweak, (plan) =>
      setConfirming((current) =>
        current && current.tweak.id === tweak.id && current.kind === kind
          ? { ...current, plan }
          : current,
      ),
    );
  };

  const toggle = (tweak: TweakInfo, value: boolean) => {
    if (value) {
      if (tweak.risk === 'safe') void apply(tweak);
      else confirm('apply', tweak);
    } else if (tweak.state.kind === 'applied' && !tweak.appliedByVolt) {
      confirm('reset', tweak);
    } else {
      void revert(tweak, false);
    }
  };

  const changeMode = (value: Mode) => {
    setMode(value);
    writeMode(value);
  };

  const tweaks = load.status === 'ready' ? load.tweaks : [];
  const detailed = mode === 'advanced';
  const inMode = useMemo(
    () =>
      detailed ? tweaks : tweaks.filter((tweak) => tweak.risk === 'safe' && !tweak.needsAdmin),
    [tweaks, detailed],
  );
  const categories = useMemo(
    () => CATEGORY_ORDER.filter((name) => inMode.some((tweak) => tweak.category === name)),
    [inMode],
  );
  const activeCategory: CategoryFilter = categories.includes(category as TweakCategory)
    ? category
    : 'all';
  const visible = useMemo(() => {
    const needle = query.trim().toLowerCase();
    return inMode.filter(
      (tweak) =>
        (activeCategory === 'all' || tweak.category === activeCategory) &&
        (!needle ||
          `${tweakTitle(tweak)} ${tweakDescription(tweak)} ${tweak.risk}`
            .toLowerCase()
            .includes(needle)),
    );
  }, [inMode, query, activeCategory]);
  const groups = CATEGORY_ORDER.map((name) => ({
    name,
    items: visible.filter((tweak) => tweak.category === name),
  })).filter((group) => group.items.length > 0);
  const hidden = tweaks.length - inMode.length;

  const confirmTitle =
    confirming?.kind === 'reset'
      ? t('tweaks.confirm.titleOff')
      : t(
          confirming?.tweak.risk === 'caution'
            ? 'tweaks.confirm.titleCaution'
            : 'tweaks.confirm.titleAdvanced',
        );
  const closeConfirm = () => setConfirming(null);

  return (
    <>
      <PageHeader title={t('nav.tweaks')} description={t('tweaks.description')} />
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
            <p>{t('tweaks.loading')}</p>
          </Card>
        ) : null}

        {load.status === 'error' ? (
          <Card role="alert" className="flex flex-col items-center gap-3 py-10 text-center">
            <TriangleAlert aria-hidden size={24} className="text-muted" />
            <p className="font-semibold">{t('tweaks.error')}</p>
            <p className="max-w-xs text-body text-muted">{load.message}</p>
            <Button onClick={read}>{t('common.tryAgain')}</Button>
          </Card>
        ) : null}

        {load.status === 'ready' && tweaks.length === 0 ? (
          <Card className="flex flex-col items-center gap-2 py-10 text-center">
            <SlidersHorizontal aria-hidden size={24} className="text-muted" />
            <p className="font-semibold">{t('tweaks.none.title')}</p>
            <p className="text-body text-muted">{t('tweaks.none.description')}</p>
          </Card>
        ) : null}

        {load.status === 'ready' && tweaks.length > 0 ? (
          <>
            <div className="flex flex-col gap-2">
              <SegmentedControl<Mode>
                label={t('tweaks.mode.label')}
                options={[
                  { value: 'simple', label: t('tweaks.mode.simple') },
                  { value: 'advanced', label: t('tweaks.mode.advanced') },
                ]}
                value={mode}
                onValueChange={changeMode}
              />
              <SearchField
                label={t('tweaks.search')}
                placeholder={t('tweaks.search')}
                value={query}
                onValueChange={setQuery}
              />
              {categories.length > 1 ? (
                <div className="overflow-x-auto">
                  <SegmentedControl<CategoryFilter>
                    label={t('tweaks.category.label')}
                    options={[
                      { value: 'all', label: t('tweaks.category.all') },
                      ...categories.map((name) => ({ value: name, label: categoryLabel(name) })),
                    ]}
                    value={activeCategory}
                    onValueChange={setCategory}
                  />
                </div>
              ) : null}
            </div>
            {groups.length > 0 ? (
              groups.map((group) => (
                <section key={group.name} aria-label={categoryLabel(group.name)}>
                  <h2 className="mb-1.5 px-1 text-caption font-semibold tracking-wide text-muted uppercase">
                    {categoryLabel(group.name)}
                  </h2>
                  <ListGroup
                    aria-label={t('tweaks.group.label', { category: categoryLabel(group.name) })}
                  >
                    {group.items.map((tweak) => (
                      <TweakRow
                        key={tweak.id}
                        tweak={tweak}
                        detailed={detailed}
                        busy={busy[tweak.id]}
                        plan={plans[tweak.id]}
                        onToggle={toggle}
                        onUndo={(target) => void revert(target, true)}
                        onShowPlan={showPlan}
                      />
                    ))}
                  </ListGroup>
                </section>
              ))
            ) : (
              <Card className="flex flex-col items-center gap-2 py-10 text-center">
                <SearchX aria-hidden size={24} className="text-muted" />
                <p className="font-semibold">
                  {t('tweaks.noMatch.title', { query: query.trim() })}
                </p>
                <p className="text-body text-muted">{t('tweaks.noMatch.hint')}</p>
              </Card>
            )}
            {!detailed && hidden > 0 ? (
              <p className="px-1 text-caption text-muted">{tn('tweaks.hidden', hidden)}</p>
            ) : null}
          </>
        ) : null}
      </div>
      <Dialog
        open={confirming !== null}
        onClose={closeConfirm}
        title={confirmTitle}
        description={
          confirming?.kind === 'reset'
            ? t('tweaks.confirm.descriptionOff', { title: tweakTitle(confirming.tweak) })
            : confirming
              ? t('tweaks.confirm.descriptionOn', { title: tweakTitle(confirming.tweak) })
              : undefined
        }
        initialFocusRef={cancelRef}
        footer={
          <>
            <Button ref={cancelRef} onClick={closeConfirm}>
              {t('common.cancel')}
            </Button>
            <Button
              variant="primary"
              disabled={
                confirming?.plan.status !== 'ready' ||
                (confirming.kind === 'reset' && confirming.plan.plan.reset === null)
              }
              onClick={() => {
                const current = confirming;
                setConfirming(null);
                if (!current) return;
                void (current.kind === 'reset' ? reset(current.tweak) : apply(current.tweak));
              }}
            >
              {confirming?.kind === 'reset'
                ? t('tweaks.confirm.turnOff')
                : t('tweaks.confirm.turnOn')}
            </Button>
          </>
        }
      >
        {confirming?.plan.status === 'loading' ? (
          <p className="text-body text-muted">{t('tweaks.confirm.reading')}</p>
        ) : null}
        {confirming?.plan.status === 'error' ? (
          <p className="text-body text-muted">{confirming.plan.message}</p>
        ) : null}
        {confirming?.plan.status === 'ready' ? (
          <div className="max-h-64 overflow-y-auto text-body">
            {confirming.kind === 'apply' ? (
              <>
                <p className="font-semibold">{t('tweaks.confirm.willChange')}</p>
                <StepList
                  label={t('tweaks.confirm.changesLabel')}
                  steps={confirming.plan.plan.steps.filter((step) => !step.atTarget)}
                />
              </>
            ) : confirming.plan.plan.reset ? (
              <>
                <p className="font-semibold">{t('tweaks.confirm.willWrite')}</p>
                <p className="text-caption text-muted">{t('tweaks.confirm.resetNote')}</p>
                <StepList
                  label={t('tweaks.confirm.changesLabel')}
                  steps={confirming.plan.plan.reset}
                />
              </>
            ) : (
              <p className="text-muted">{t('tweaks.confirm.cannotReset')}</p>
            )}
          </div>
        ) : null}
      </Dialog>
    </>
  );
}
