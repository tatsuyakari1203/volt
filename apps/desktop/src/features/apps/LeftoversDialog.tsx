import { Loader2 } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { say, t, tn } from '../../shared/i18n';
import {
  type LeftoverRemovalView,
  type LeftoverScanView,
  type LeftoverView,
  removeLeftovers,
  scanLeftovers,
} from '../../shared/ipc';
import { describeError } from '../../shared/text/errors';
import { formatSize } from '../../shared/text/size';
import { Badge, Button, Dialog } from '../../shared/ui';
import {
  chosenItems,
  confidenceText,
  defaultSelection,
  evidenceTexts,
  lockedText,
  totalBytes,
} from './leftovers';

export interface LeftoversDialogProps {
  /** The app whose leftovers are looked for, or `null` while the dialog is closed. */
  app: { id: string; name: string } | null;
  onClose: () => void;
}

type Stage =
  | { kind: 'scanning' }
  | { kind: 'failed'; message: string }
  | { kind: 'list'; scan: LeftoverScanView }
  | { kind: 'confirm'; scan: LeftoverScanView }
  | { kind: 'removing'; scan: LeftoverScanView }
  | { kind: 'done'; removal: LeftoverRemovalView };

/** How much a find holds: a folder's size and files, or the keys of a registry tree. */
function holdsText(item: LeftoverView): string {
  if (item.kind === 'folder') {
    return `${formatSize(item.sizeBytes)} · ${tn('count.file', item.items)}`;
  }
  // A machine-wide key is not read, so there is nothing to count.
  return item.items > 0 ? tn('count.key', item.items) : '';
}

function Row({
  item,
  ticked,
  onToggle,
}: {
  item: LeftoverView;
  ticked: boolean;
  onToggle: (id: string, on: boolean) => void;
}) {
  const locked = lockedText(item);
  const holds = holdsText(item);
  return (
    <li>
      <label className="flex items-start gap-3 px-1 py-2">
        <input
          type="checkbox"
          className="mt-1 size-4 shrink-0 accent-(--color-accent)"
          aria-label={t('apps.leftovers.select', { location: item.location })}
          checked={ticked && locked === null}
          disabled={locked !== null}
          onChange={(event) => onToggle(item.id, event.target.checked)}
        />
        <div className="min-w-0 flex-1">
          <p className="wrap-anywhere font-mono text-caption font-semibold">{item.location}</p>
          <div className="mt-1 flex flex-wrap items-center gap-1">
            <Badge risk={item.confidence === 'high' ? 'safe' : undefined}>
              {confidenceText(item.confidence)}
            </Badge>
            <Badge>
              {item.kind === 'folder'
                ? t('apps.leftovers.kind.folder')
                : t('apps.leftovers.kind.key')}
            </Badge>
            {locked === null ? null : <Badge risk="caution">{t('apps.leftovers.locked')}</Badge>}
            {holds ? <span className="text-caption text-muted">{holds}</span> : null}
          </div>
          <ul className="mt-1 list-disc pl-4 text-caption text-muted">
            {evidenceTexts(item).map((text) => (
              <li key={text}>{text}</li>
            ))}
            {locked === null ? null : <li>{locked}</li>}
          </ul>
        </div>
      </label>
    </li>
  );
}

/**
 * Looks for what an uninstaller left behind, after the user asked for it (never on its own), and
 * removes what the user ticks. Every find shows its confidence and why Volt thinks it belongs to
 * the app; only high confidence is ticked at first, and machine-wide keys are shown but locked.
 * Removing asks once more with the sizes: folders go to the Recycle Bin, registry keys are saved in
 * the undo journal first. What may be offered and removed is decided by the Rust core.
 */
export function LeftoversDialog({ app, onClose }: LeftoversDialogProps) {
  const [stage, setStage] = useState<Stage>({ kind: 'scanning' });
  const [ticked, setTicked] = useState<ReadonlySet<string>>(new Set());
  const alive = useRef(true);

  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  const appId = app?.id ?? null;
  useEffect(() => {
    if (appId === null) return;
    let current = true;
    setStage({ kind: 'scanning' });
    scanLeftovers(appId)
      .then((scan) => {
        if (!current) return;
        setTicked(defaultSelection(scan));
        setStage({ kind: 'list', scan });
      })
      .catch((error: unknown) => {
        if (current) setStage({ kind: 'failed', message: describeError(error) });
      });
    return () => {
      current = false;
    };
  }, [appId]);

  if (app === null) return null;

  const toggle = (id: string, on: boolean) =>
    setTicked((previous) => {
      const next = new Set(previous);
      if (on) next.add(id);
      else next.delete(id);
      return next;
    });

  const remove = (scan: LeftoverScanView) => {
    const ids = chosenItems(scan, ticked).map((item) => item.id);
    setStage({ kind: 'removing', scan });
    removeLeftovers(app.id, ids)
      .then((removal) => {
        if (alive.current) setStage({ kind: 'done', removal });
      })
      .catch((error: unknown) => {
        if (alive.current) setStage({ kind: 'failed', message: describeError(error) });
      });
  };

  const title = t('apps.leftovers.title', { name: app.name });
  const close = <Button onClick={onClose}>{t('apps.leftovers.close')}</Button>;

  if (stage.kind === 'scanning' || stage.kind === 'removing') {
    return (
      <Dialog
        open
        onClose={() => {}}
        title={title}
        footer={<Button disabled>{t('apps.leftovers.close')}</Button>}
      >
        <p role="status" className="flex items-center gap-2 text-muted">
          <Loader2 aria-hidden size={16} className="animate-spin motion-reduce:animate-none" />
          {stage.kind === 'scanning' ? t('apps.leftovers.scanning') : t('apps.leftovers.removing')}
        </p>
      </Dialog>
    );
  }

  if (stage.kind === 'failed') {
    return (
      <Dialog open onClose={onClose} title={title} footer={close}>
        <p role="alert">{t('apps.leftovers.error', { error: stage.message })}</p>
      </Dialog>
    );
  }

  if (stage.kind === 'done') {
    const { removal } = stage;
    return (
      <Dialog
        open
        onClose={onClose}
        title={t('apps.leftovers.done.title')}
        description={t('apps.leftovers.done.summary', {
          items: tn('count.item', removal.removed.length),
          size: formatSize(removal.removedBytes),
        })}
        footer={
          <Button variant="primary" onClick={onClose}>
            {t('apps.leftovers.close')}
          </Button>
        }
      >
        {removal.skipped.length > 0 ? (
          <>
            <p className="font-semibold">{t('apps.leftovers.done.skipped')}</p>
            <ul className="mt-1 max-h-32 list-disc overflow-y-auto pl-5 text-caption">
              {removal.skipped.map((skip) => (
                <li key={skip.id} className="wrap-anywhere">
                  {skip.id.replace(/^(folder|key):/, '')}: {say(skip.reasonPhrase, skip.reason)}
                </li>
              ))}
            </ul>
          </>
        ) : null}
        {removal.removed.some((item) => item.kind === 'registryKey') ? (
          <p className="mt-3 text-caption text-muted">{t('apps.leftovers.done.undo')}</p>
        ) : null}
      </Dialog>
    );
  }

  const { scan } = stage;
  const chosen = chosenItems(scan, ticked);

  if (stage.kind === 'confirm') {
    return (
      <Dialog
        open
        onClose={onClose}
        title={t('apps.leftovers.confirm.title')}
        description={t('apps.leftovers.confirm.description')}
        footer={
          <>
            <Button onClick={() => setStage({ kind: 'list', scan })}>
              {t('apps.leftovers.back')}
            </Button>
            <Button variant="primary" onClick={() => remove(scan)}>
              {t('apps.leftovers.confirm.remove')}
            </Button>
          </>
        }
      >
        <ul
          aria-label={t('apps.leftovers.confirm.list.label')}
          className="max-h-56 list-disc overflow-y-auto pl-5 text-body"
        >
          {chosen.map((item) => (
            <li key={item.id} className="wrap-anywhere">
              {item.location}
              <span className="text-caption text-muted"> · {holdsText(item)}</span>
            </li>
          ))}
        </ul>
        <p className="mt-3 text-caption text-muted">
          {t('apps.leftovers.confirm.total', {
            items: tn('count.item', chosen.length),
            size: formatSize(totalBytes(chosen)),
          })}
        </p>
      </Dialog>
    );
  }

  return (
    <Dialog
      open
      onClose={onClose}
      title={title}
      description={
        scan.items.length > 0
          ? t('apps.leftovers.description', { name: app.name })
          : t('apps.leftovers.none', { name: app.name })
      }
      footer={
        <>
          {close}
          {scan.items.length > 0 ? (
            <Button
              variant="primary"
              disabled={chosen.length === 0}
              onClick={() => setStage({ kind: 'confirm', scan })}
            >
              {t('apps.leftovers.review')}
            </Button>
          ) : null}
        </>
      }
    >
      {scan.items.length > 0 ? (
        <>
          <ul
            aria-label={t('apps.leftovers.list.label')}
            className="max-h-[55vh] divide-y divide-line overflow-y-auto"
          >
            {scan.items.map((item) => (
              <Row key={item.id} item={item} ticked={ticked.has(item.id)} onToggle={toggle} />
            ))}
          </ul>
          <p className="mt-3 text-caption text-muted" aria-live="polite">
            {t('apps.leftovers.selection', { items: tn('count.item', chosen.length) })}
          </p>
        </>
      ) : null}
      {scan.incomplete ? (
        <p className="mt-2 text-caption text-muted">{t('apps.leftovers.incomplete')}</p>
      ) : null}
    </Dialog>
  );
}
