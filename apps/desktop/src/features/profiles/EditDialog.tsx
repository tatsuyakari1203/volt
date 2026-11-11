import { useId, useRef, useState } from 'react';
import { t, tx } from '../../shared/i18n';
import {
  type AutomationConfigView,
  deleteProfile,
  type ProfilesView,
  type ProfileView,
  saveProfile,
  type TweakInfo,
} from '../../shared/ipc';
import { Badge, Button, Dialog } from '../../shared/ui';
import { describeError } from './messages';

type TweakMode = 'none' | 'on' | 'off';
type PartMode = 'leave' | 'on' | 'off';

const fieldClass =
  'h-9 rounded-md border border-line-strong bg-surface px-2 text-fg transition-colors duration-(--duration-fast) hover:bg-surface-hover focus:border-accent disabled:opacity-50';

export interface EditDialogProps {
  /** The profile to edit, or `null` for a new one. */
  profile: ProfileView | null;
  /** Every tweak of the catalogue. Those that need administrator rights cannot be chosen. */
  tweaks: readonly TweakInfo[];
  /** What automation is set to now: the numbers a part keeps when it is turned on or off. */
  automation: AutomationConfigView | null;
  /** Seconds between two looks at the running apps, for the cost line. */
  pollSeconds: number;
  maxNameChars: number;
  onClose: () => void;
  /** Called with the new list after a save or a delete. */
  onChanged: (view: ProfilesView) => void;
}

function partMode(part: { enabled: boolean } | null | undefined): PartMode {
  if (!part) return 'leave';
  return part.enabled ? 'on' : 'off';
}

/** Creates or edits a profile: its name, the tweaks it turns on or off, the app that activates it and what it sets in automation. */
export function EditDialog({
  profile,
  tweaks,
  automation,
  pollSeconds,
  maxNameChars,
  onClose,
  onChanged,
}: EditDialogProps) {
  const ids = useId();
  const nameRef = useRef<HTMLInputElement>(null);
  const [name, setName] = useState(profile?.name ?? '');
  const [modes, setModes] = useState<Record<string, TweakMode>>(() => {
    const start: Record<string, TweakMode> = {};
    for (const entry of profile?.tweaks ?? []) start[entry.id] = entry.enabled ? 'on' : 'off';
    return start;
  });
  const [exe, setExe] = useState(profile?.trigger?.exe ?? '');
  const [back, setBack] = useState(profile?.trigger?.returnOnExit ?? true);
  const [memory, setMemory] = useState<PartMode>(partMode(profile?.automation.memory));
  const [cleaning, setCleaning] = useState<PartMode>(partMode(profile?.automation.cleaning));
  const [busy, setBusy] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const canSetAutomation = automation !== null;

  async function save() {
    setBusy(true);
    setError(null);
    const memoryBase = profile?.automation.memory ?? automation?.memory ?? null;
    const cleaningBase = profile?.automation.cleaning ?? automation?.cleaning ?? null;
    const next: ProfileView = {
      id: profile?.id ?? '',
      name: name.trim(),
      tweaks: tweaks
        .filter((tweak) => !tweak.needsAdmin && (modes[tweak.id] ?? 'none') !== 'none')
        .map((tweak) => ({ id: tweak.id, enabled: modes[tweak.id] === 'on' })),
      automation: {
        memory:
          memory === 'leave' || !memoryBase ? null : { ...memoryBase, enabled: memory === 'on' },
        cleaning:
          cleaning === 'leave' || !cleaningBase
            ? null
            : { ...cleaningBase, enabled: cleaning === 'on' },
      },
      trigger: exe.trim() === '' ? null : { exe: exe.trim(), returnOnExit: back },
    };
    try {
      onChanged(await saveProfile(next));
    } catch (e) {
      setError(t('profiles.edit.saveFailed', { message: describeError(e) }));
      setBusy(false);
    }
  }

  async function remove() {
    if (!profile) return;
    setBusy(true);
    setError(null);
    try {
      onChanged(await deleteProfile(profile.id));
    } catch (e) {
      setError(t('profiles.edit.deleteFailed', { message: describeError(e) }));
      setBusy(false);
    }
  }

  return (
    <Dialog
      open
      onClose={busy ? () => {} : onClose}
      title={
        profile ? t('profiles.edit.titleEdit', { name: profile.name }) : t('profiles.edit.titleNew')
      }
      initialFocusRef={nameRef}
      footer={
        <>
          {profile ? (
            <Button
              className="mr-auto"
              disabled={busy}
              onClick={() => (confirmDelete ? void remove() : setConfirmDelete(true))}
            >
              {confirmDelete
                ? t('profiles.edit.deleteConfirm', { name: profile.name })
                : t('profiles.edit.delete')}
            </Button>
          ) : null}
          <Button disabled={busy} onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button
            variant="primary"
            loading={busy}
            disabled={name.trim() === ''}
            onClick={() => void save()}
          >
            {t('profiles.edit.save')}
          </Button>
        </>
      }
    >
      <div className="flex max-h-[60vh] flex-col gap-4 overflow-y-auto pr-1">
        {error ? (
          <p role="alert" className="text-body text-muted">
            {error}
          </p>
        ) : null}
        {confirmDelete ? (
          <p role="note" className="text-caption text-muted">
            {t('profiles.edit.deleteNote')}
          </p>
        ) : null}
        <label className="flex flex-col gap-1 text-body" htmlFor={`${ids}-name`}>
          <span className="font-semibold">{t('profiles.edit.name')}</span>
          <input
            id={`${ids}-name`}
            ref={nameRef}
            value={name}
            maxLength={maxNameChars}
            onChange={(event) => setName(event.target.value)}
            className={fieldClass}
          />
        </label>

        <fieldset className="flex flex-col gap-1">
          <legend className="font-semibold">{t('profiles.edit.tweaks')}</legend>
          <p className="text-caption text-muted">{t('profiles.edit.tweaks.hint')}</p>
          <ul className="mt-1 max-h-56 divide-y divide-line overflow-y-auto rounded-md border border-line">
            {tweaks.map((tweak) => {
              const title = tx(`tweak.${tweak.id}.title`, tweak.title);
              return (
                <li key={tweak.id} className="flex items-center gap-2 px-2 py-1.5 text-body">
                  <span className="min-w-0 flex-1">
                    <span className="flex flex-wrap items-center gap-x-2">
                      {title}
                      {tweak.risk !== 'safe' ? <Badge risk={tweak.risk} /> : null}
                    </span>
                    {tweak.needsAdmin ? (
                      <span className="block text-caption text-muted">
                        {t('profiles.edit.tweak.admin')}
                      </span>
                    ) : null}
                  </span>
                  <select
                    aria-label={t('profiles.edit.tweak.label', { title })}
                    disabled={tweak.needsAdmin}
                    value={tweak.needsAdmin ? 'none' : (modes[tweak.id] ?? 'none')}
                    onChange={(event) =>
                      setModes({ ...modes, [tweak.id]: event.target.value as TweakMode })
                    }
                    className={fieldClass}
                  >
                    <option value="none">{t('profiles.edit.mode.none')}</option>
                    <option value="on">{t('profiles.edit.mode.on')}</option>
                    <option value="off">{t('profiles.edit.mode.off')}</option>
                  </select>
                </li>
              );
            })}
          </ul>
        </fieldset>

        <fieldset className="flex flex-col gap-2">
          <legend className="font-semibold">{t('profiles.edit.automation')}</legend>
          <p className="text-caption text-muted">{t('profiles.edit.automation.hint')}</p>
          {(
            [
              ['memory', 'profiles.edit.automation.memory', memory, setMemory],
              ['cleaning', 'profiles.edit.automation.cleaning', cleaning, setCleaning],
            ] as const
          ).map(([key, label, value, set]) => (
            <label key={key} className="flex items-center justify-between gap-3 text-body">
              <span>{t(label)}</span>
              <select
                value={value}
                onChange={(event) => set(event.target.value as PartMode)}
                className={fieldClass}
              >
                <option value="leave">{t('profiles.edit.part.leave')}</option>
                <option value="on" disabled={!canSetAutomation && value === 'leave'}>
                  {t('profiles.edit.part.on')}
                </option>
                <option value="off" disabled={!canSetAutomation && value === 'leave'}>
                  {t('profiles.edit.part.off')}
                </option>
              </select>
            </label>
          ))}
        </fieldset>

        <fieldset className="flex flex-col gap-2">
          <legend className="font-semibold">{t('profiles.edit.trigger')}</legend>
          <label className="flex flex-col gap-1 text-body" htmlFor={`${ids}-exe`}>
            <span>{t('profiles.edit.trigger.exe')}</span>
            <input
              id={`${ids}-exe`}
              value={exe}
              placeholder="game.exe"
              autoCapitalize="none"
              spellCheck={false}
              onChange={(event) => setExe(event.target.value)}
              className={fieldClass}
            />
          </label>
          <label className="flex items-center gap-2 text-body">
            <input
              type="checkbox"
              checked={back}
              disabled={exe.trim() === ''}
              onChange={(event) => setBack(event.target.checked)}
            />
            {t('profiles.edit.trigger.back')}
          </label>
          <p className="text-caption text-muted">
            {t('profiles.watch.note', { seconds: pollSeconds })}
          </p>
        </fieldset>
      </div>
    </Dialog>
  );
}
