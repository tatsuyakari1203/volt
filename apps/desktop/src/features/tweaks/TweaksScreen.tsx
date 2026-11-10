import { Loader2, SearchX, SlidersHorizontal, TriangleAlert } from 'lucide-react';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  applyTweak,
  listTweaks,
  revertTweak,
  type TweakInfo,
  type TweakStateView,
} from '../../shared/ipc';
import {
  Badge,
  Button,
  Card,
  Dialog,
  ListGroup,
  ListRow,
  PageHeader,
  SearchField,
  Switch,
  useToast,
} from '../../shared/ui';
import {
  describeApply,
  describeError,
  describeFailure,
  describeRevert,
  type Outcome,
  restartNote,
} from './messages';

type Load =
  | { status: 'loading' }
  | { status: 'error'; message: string }
  | { status: 'ready'; tweaks: TweakInfo[] };

/** What a tweak is doing right now, while a command runs. */
type Busy = 'applying' | 'undoing';

function evidenceLine(tweak: TweakInfo): string {
  const label = tweak.evidence === 'measured' ? 'Measured' : 'Anecdotal';
  return `Evidence: ${label}. ${tweak.evidenceNote}${restartNote(tweak.restart)}`;
}

function stateLine(tweak: TweakInfo, busy: Busy | undefined): string | null {
  if (busy === 'applying') return 'Turning on...';
  if (busy === 'undoing') return 'Undoing...';
  if (tweak.needsAdmin) return 'Needs administrator rights. It cannot be changed from here yet.';
  const state = tweak.state;
  switch (state.kind) {
    case 'partial':
      return 'Only partly on. Turn it on to finish, or use Undo to put things back.';
    case 'unreadable':
      return `Could not read the current setting. ${state.message}`;
    default:
      return null;
  }
}

/**
 * The Tweaks screen: the catalogue tweaks with their state read live from the system, a switch
 * to turn each on (apply) or off (revert), and a toast with Undo after every change. The state
 * is read again whenever the screen is mounted, which includes a freshly created window, so it
 * is never older than the window itself. All rules live in the Rust core: this renders what the
 * commands report.
 */
export function TweaksScreen() {
  const toast = useToast();
  const [load, setLoad] = useState<Load>({ status: 'loading' });
  const [query, setQuery] = useState('');
  const [busy, setBusy] = useState<Readonly<Record<string, Busy>>>({});
  const [notice, setNotice] = useState<string | null>(null);
  const [confirming, setConfirming] = useState<TweakInfo | null>(null);
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

  const patchState = (id: string, state: TweakStateView) => {
    setLoad((current) =>
      current.status === 'ready'
        ? {
            status: 'ready',
            tweaks: current.tweaks.map((tweak) => (tweak.id === id ? { ...tweak, state } : tweak)),
          }
        : current,
    );
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
        ? { actionLabel: 'Undo', onAction: () => void revert(tweak, true) }
        : {}),
    });
  };

  const apply = async (tweak: TweakInfo) => {
    setBusyFor(tweak.id, 'applying');
    setNotice(null);
    try {
      const result = await applyTweak(tweak.id);
      if (alive.current) patchState(tweak.id, result.state);
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
      if (alive.current) patchState(tweak.id, result.state);
      report(describeRevert(tweak, result, afterUndo), tweak);
    } catch (error) {
      if (alive.current) setNotice(describeFailure('undo', tweak, error));
    } finally {
      setBusyFor(tweak.id, undefined);
    }
  };

  const toggle = (tweak: TweakInfo, value: boolean) => {
    if (value && tweak.risk === 'advanced') {
      setConfirming(tweak);
      return;
    }
    void (value ? apply(tweak) : revert(tweak, false));
  };

  const tweaks = load.status === 'ready' ? load.tweaks : [];
  const visible = useMemo(() => {
    const needle = query.trim().toLowerCase();
    if (!needle) return tweaks;
    return tweaks.filter((tweak) =>
      `${tweak.title} ${tweak.description} ${tweak.risk}`.toLowerCase().includes(needle),
    );
  }, [tweaks, query]);

  return (
    <>
      <PageHeader
        title="Tweaks"
        description="Volt records the old setting before each change, so you can undo it."
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
            <p>Reading your settings...</p>
          </Card>
        ) : null}

        {load.status === 'error' ? (
          <Card role="alert" className="flex flex-col items-center gap-3 py-10 text-center">
            <TriangleAlert aria-hidden size={24} className="text-muted" />
            <p className="font-semibold">Tweaks could not be loaded</p>
            <p className="max-w-xs text-body text-muted">{load.message}</p>
            <Button onClick={read}>Try again</Button>
          </Card>
        ) : null}

        {load.status === 'ready' && tweaks.length === 0 ? (
          <Card className="flex flex-col items-center gap-2 py-10 text-center">
            <SlidersHorizontal aria-hidden size={24} className="text-muted" />
            <p className="font-semibold">No tweaks yet</p>
            <p className="text-body text-muted">This version of Volt has no tweaks to offer.</p>
          </Card>
        ) : null}

        {load.status === 'ready' && tweaks.length > 0 ? (
          <>
            <SearchField
              label="Search tweaks"
              placeholder="Search tweaks"
              value={query}
              onValueChange={setQuery}
            />
            {visible.length > 0 ? (
              <ListGroup aria-label="Tweaks">
                {visible.map((tweak) => {
                  const working = busy[tweak.id];
                  const status = stateLine(tweak, working);
                  return (
                    <ListRow
                      key={tweak.id}
                      title={tweak.title}
                      description={
                        <>
                          <span className="block">{tweak.description}</span>
                          <span className="mt-0.5 block">{evidenceLine(tweak)}</span>
                          {status ? (
                            <span className="mt-0.5 block font-semibold text-fg">{status}</span>
                          ) : null}
                        </>
                      }
                      badge={<Badge risk={tweak.risk} />}
                      trailing={
                        <div className="flex items-center gap-2">
                          {tweak.state.kind === 'partial' && !tweak.needsAdmin ? (
                            <Button
                              size="sm"
                              aria-label={`Undo ${tweak.title}`}
                              disabled={working !== undefined}
                              onClick={() => void revert(tweak, true)}
                            >
                              Undo
                            </Button>
                          ) : null}
                          <Switch
                            aria-label={tweak.title}
                            aria-busy={working !== undefined}
                            checked={tweak.state.kind === 'applied'}
                            disabled={
                              working !== undefined ||
                              tweak.needsAdmin ||
                              tweak.state.kind === 'unreadable'
                            }
                            onCheckedChange={(value) => toggle(tweak, value)}
                          />
                        </div>
                      }
                    />
                  );
                })}
              </ListGroup>
            ) : (
              <Card className="flex flex-col items-center gap-2 py-10 text-center">
                <SearchX aria-hidden size={24} className="text-muted" />
                <p className="font-semibold">No tweaks match "{query.trim()}"</p>
                <p className="text-body text-muted">Try a different word.</p>
              </Card>
            )}
          </>
        ) : null}
      </div>
      <Dialog
        open={confirming !== null}
        onClose={() => setConfirming(null)}
        title="Turn on an Advanced tweak?"
        description={
          confirming
            ? `"${confirming.title}" can change how Windows behaves. Volt records the old value so you can undo it.`
            : undefined
        }
        initialFocusRef={cancelRef}
        footer={
          <>
            <Button ref={cancelRef} onClick={() => setConfirming(null)}>
              Cancel
            </Button>
            <Button
              variant="primary"
              onClick={() => {
                const tweak = confirming;
                setConfirming(null);
                if (tweak) void apply(tweak);
              }}
            >
              Turn on
            </Button>
          </>
        }
      />
    </>
  );
}
