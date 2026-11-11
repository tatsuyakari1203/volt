import { ListPlus, ShieldCheck, X } from 'lucide-react';
import { type FormEvent, useCallback, useEffect, useRef, useState } from 'react';
import { t } from '../../shared/i18n';
import {
  addMemoryExclusion,
  getMemoryExclusions,
  listProcesses,
  type ProcessGroupView,
  removeMemoryExclusion,
} from '../../shared/ipc';
import { describeError } from '../../shared/text/memory';
import { formatBytes } from '../../shared/text/size';
import { Button, Card } from '../../shared/ui';

/** How many running programs the picker lists: the biggest memory users, which are the ones worth protecting. */
const PICKER_ROWS = 40;

/**
 * The programs protected from memory trimming. The user adds one by typing an executable name or
 * by picking from the running programs; the list is stored by the shell and every trim (Boost, the
 * clean on this page, the tray Boost) skips what is on it. The rules (name matching, skipping)
 * live in the Rust core: this only edits the list.
 */
export function ExclusionsCard() {
  const [names, setNames] = useState<string[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [typed, setTyped] = useState('');
  const [busy, setBusy] = useState(false);
  const [picking, setPicking] = useState(false);
  const [running, setRunning] = useState<ProcessGroupView[] | null>(null);
  const alive = useRef(true);

  useEffect(() => {
    alive.current = true;
    getMemoryExclusions()
      .then((list) => {
        if (alive.current) setNames(list);
      })
      .catch((failure: unknown) => {
        if (alive.current) setError(describeError(failure));
      });
    return () => {
      alive.current = false;
    };
  }, []);

  const change = useCallback(async (action: () => Promise<string[]>): Promise<boolean> => {
    setBusy(true);
    setError(null);
    try {
      const list = await action();
      if (alive.current) setNames(list);
      return true;
    } catch (failure) {
      if (alive.current) setError(describeError(failure));
      return false;
    } finally {
      if (alive.current) setBusy(false);
    }
  }, []);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (typed.trim() === '') return;
    if (await change(() => addMemoryExclusion(typed))) setTyped('');
  };

  const openPicker = () => {
    const next = !picking;
    setPicking(next);
    if (next) {
      listProcesses()
        .then((list) => {
          if (alive.current) setRunning(list.groups);
        })
        .catch((failure: unknown) => {
          if (alive.current) {
            setRunning([]);
            setError(describeError(failure));
          }
        });
    }
  };

  const protectedNames = new Set(names ?? []);
  const choices = (running ?? [])
    .filter((group) => !protectedNames.has(group.name.toLowerCase()))
    .slice(0, PICKER_ROWS);

  return (
    <Card as="section" aria-label={t('memory.exclusions.label')} className="flex flex-col gap-3">
      <div>
        <h2 className="text-subtitle font-semibold">{t('memory.exclusions.title')}</h2>
        <p className="text-body text-muted">{t('memory.exclusions.description')}</p>
      </div>

      {error ? (
        <p role="alert" className="rounded-md bg-caution p-2 text-body text-caution-fg">
          {error}
        </p>
      ) : null}

      {names && names.length === 0 ? (
        <p className="text-body text-muted">{t('memory.exclusions.empty')}</p>
      ) : null}
      {names && names.length > 0 ? (
        <ul aria-label={t('memory.exclusions.listLabel')} className="flex flex-col gap-1">
          {names.map((name) => (
            <li
              key={name}
              className="flex items-center justify-between gap-2 rounded-md border border-line px-3 py-1"
            >
              <span className="flex min-w-0 items-center gap-2">
                <ShieldCheck aria-hidden size={16} className="shrink-0 text-muted" />
                <span className="truncate font-semibold">{name}</span>
              </span>
              <Button
                size="sm"
                variant="ghost"
                disabled={busy}
                aria-label={t('memory.exclusions.remove', { name })}
                icon={<X aria-hidden size={16} />}
                onClick={() => void change(() => removeMemoryExclusion(name))}
              >
                {t('memory.exclusions.removeShort')}
              </Button>
            </li>
          ))}
        </ul>
      ) : null}

      <form className="flex gap-2" onSubmit={(event) => void submit(event)}>
        <label className="min-w-0 flex-1">
          <span className="sr-only">{t('memory.exclusions.inputLabel')}</span>
          <input
            type="text"
            value={typed}
            onChange={(event) => setTyped(event.target.value)}
            placeholder={t('memory.exclusions.placeholder')}
            autoComplete="off"
            spellCheck={false}
            className="h-9 w-full rounded-md border border-line-strong bg-surface px-3 text-fg transition-colors duration-(--duration-fast) placeholder:text-muted hover:bg-surface-hover focus:border-accent"
          />
        </label>
        <Button type="submit" disabled={busy || typed.trim() === ''}>
          {t('memory.exclusions.add')}
        </Button>
      </form>

      <div>
        <Button
          size="sm"
          icon={<ListPlus aria-hidden size={16} />}
          aria-expanded={picking}
          onClick={openPicker}
        >
          {picking ? t('memory.exclusions.hidePicker') : t('memory.exclusions.showPicker')}
        </Button>
      </div>
      {picking ? (
        running === null ? (
          <p role="status" className="text-body text-muted">
            {t('memory.exclusions.pickerLoading')}
          </p>
        ) : choices.length === 0 ? (
          <p className="text-body text-muted">{t('memory.exclusions.pickerEmpty')}</p>
        ) : (
          <ul
            aria-label={t('memory.exclusions.pickerLabel')}
            className="flex max-h-56 flex-col gap-1 overflow-y-auto"
          >
            {choices.map((group) => (
              <li
                key={group.id}
                className="flex items-center justify-between gap-2 rounded-md border border-line px-3 py-1"
              >
                <span className="min-w-0">
                  <span className="block truncate font-semibold">{group.name}</span>
                  <span className="block text-caption text-muted tabular-nums">
                    {formatBytes(group.memoryBytes)}
                  </span>
                </span>
                <Button
                  size="sm"
                  disabled={busy}
                  aria-label={t('memory.exclusions.protect', { name: group.name })}
                  onClick={() => void change(() => addMemoryExclusion(group.name))}
                >
                  {t('memory.exclusions.protectShort')}
                </Button>
              </li>
            ))}
          </ul>
        )
      ) : null}
    </Card>
  );
}
