import { Bell, CalendarClock, MemoryStick } from 'lucide-react';
import { useEffect, useState } from 'react';
import { localeTag, type MessageKey, t, tx } from '../../shared/i18n';
import {
  type AutomationConfigView,
  type AutomationStatusView,
  type CleaningRuleInfo,
  getAutomationStatus,
  listCleaningRules,
  onAutomationUpdated,
  setAutomationConfig,
} from '../../shared/ipc';
import { describeError } from '../../shared/text/memory';
import { Badge, Card, ListGroup, ListRow, Muted, SegmentedControl, Switch } from '../../shared/ui';

type Pause = NonNullable<AutomationStatusView['paused']>;

const pausedKeys: Record<Pause, MessageKey> = {
  fullscreen: 'settings.automation.paused.fullscreen',
  batterySaver: 'settings.automation.paused.batterySaver',
  manualRun: 'settings.automation.paused.manualRun',
};

const reasonKeys: Record<Pause, MessageKey> = {
  fullscreen: 'settings.automation.reason.fullscreen',
  batterySaver: 'settings.automation.reason.batterySaver',
  manualRun: 'settings.automation.reason.manualRun',
};

/** A moment in the language and time zone of the user, or "never". */
function when(ms: number | null): string {
  if (ms === null) return t('settings.automation.never');
  return new Date(ms).toLocaleString(localeTag(), { dateStyle: 'medium', timeStyle: 'short' });
}

/** The name of a weekday (0 is Monday) in the current language. */
function weekdayName(day: number): string {
  // 1 January 2024 was a Monday.
  return new Date(2024, 0, 1 + day).toLocaleDateString(localeTag(), { weekday: 'long' });
}

function timeText(minuteOfDay: number): string {
  const hours = String(Math.floor(minuteOfDay / 60)).padStart(2, '0');
  const minutes = String(minuteOfDay % 60).padStart(2, '0');
  return `${hours}:${minutes}`;
}

function parseTime(value: string): number | null {
  const match = /^(\d{1,2}):(\d{2})$/.exec(value);
  if (!match) return null;
  const minute = Number(match[1]) * 60 + Number(match[2]);
  return minute >= 0 && minute <= 1439 ? minute : null;
}

const fieldClass =
  'h-9 rounded-md border border-line-strong bg-surface px-2 text-fg transition-colors duration-(--duration-fast) hover:bg-surface-hover focus:border-accent disabled:opacity-50';

interface NumberFieldProps {
  label: string;
  unit: string;
  value: number;
  range: [number, number];
  disabled: boolean;
  onCommit: (value: number) => void;
}

/** A whole number kept inside its bounds: it is applied when the field loses focus or on Enter. */
function NumberField({ label, unit, value, range, disabled, onCommit }: NumberFieldProps) {
  const [draft, setDraft] = useState(String(value));
  useEffect(() => setDraft(String(value)), [value]);

  function commit() {
    const parsed = Number.parseInt(draft, 10);
    const next = Number.isFinite(parsed) ? Math.min(Math.max(parsed, range[0]), range[1]) : value;
    setDraft(String(next));
    if (next !== value) onCommit(next);
  }

  return (
    <label className="flex items-center justify-between gap-3 px-4 py-2 text-body">
      <span>{label}</span>
      <span className="flex items-center gap-2">
        <input
          type="number"
          inputMode="numeric"
          min={range[0]}
          max={range[1]}
          value={draft}
          disabled={disabled}
          onChange={(event) => setDraft(event.target.value)}
          onBlur={commit}
          onKeyDown={(event) => {
            if (event.key === 'Enter') commit();
          }}
          className={`${fieldClass} w-24 text-right`}
        />
        <span className="w-14 text-caption text-muted">{unit}</span>
      </span>
    </label>
  );
}

/**
 * Settings card for automation: memory cleaning on thresholds and cleaning on a schedule. Both are
 * off until the user turns them on. Every control says what it will do, the last and next runs are
 * shown, and the numbers are held inside the bounds the shell reports. A schedule can only hold
 * the rules that can run on their own (no administrator rights, no chosen folder, no picking).
 */
export function AutomationSettings() {
  const [status, setStatus] = useState<AutomationStatusView | null>(null);
  const [rules, setRules] = useState<CleaningRuleInfo[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    let unlisten: (() => void) | null = null;
    getAutomationStatus()
      .then((value) => {
        if (alive) setStatus(value);
      })
      .catch((e: unknown) => {
        if (alive) setError(t('settings.automation.readError', { message: describeError(e) }));
      });
    listCleaningRules()
      .then((list) => {
        if (alive)
          setRules(list.filter((r) => !r.needsAdministrator && !r.pickItems && !r.needsFolder));
      })
      .catch(() => {
        // Without the list the schedule shows no rules to choose from.
      });
    onAutomationUpdated((value) => {
      if (alive) setStatus(value);
    })
      .then((stop) => {
        if (alive) unlisten = stop;
        else stop();
      })
      .catch(() => {
        // Outside the Tauri window there is no event; the card still works.
      });
    return () => {
      alive = false;
      unlisten?.();
    };
  }, []);

  const config = status?.config ?? null;
  const bounds = status?.bounds ?? null;

  function save(next: AutomationConfigView) {
    setBusy(true);
    setError(null);
    setAutomationConfig(next)
      .then(setStatus)
      .catch((e: unknown) =>
        setError(t('settings.automation.saveError', { message: describeError(e) })),
      )
      .finally(() => setBusy(false));
  }

  const memory = config?.memory ?? null;
  const cleaning = config?.cleaning ?? null;
  const off = config === null || busy;
  const daysAndTime =
    cleaning === null
      ? null
      : {
          day: weekdayName(cleaning.weekday),
          time: timeText(cleaning.minuteOfDay),
        };

  return (
    <Card as="section" aria-labelledby="settings-automation" padding="none">
      <h2 id="settings-automation" className="px-4 pt-4 pb-1 text-subtitle font-semibold">
        {t('settings.automation.title')}
      </h2>
      <p className="px-4 pb-3 text-caption text-muted">{t('settings.automation.intro')}</p>
      {status?.paused ? (
        <p role="status" className="px-4 pb-3 text-caption font-semibold">
          {t(pausedKeys[status.paused])}
        </p>
      ) : null}

      <ListGroup
        aria-label={t('settings.automation.memory.title')}
        className="rounded-none border-x-0 border-b-0 shadow-none"
      >
        <ListRow
          icon={<MemoryStick aria-hidden size={18} />}
          title={t('settings.automation.memory.title')}
          description={
            memory
              ? t('settings.automation.memory.line', {
                  standby: memory.standbyAboveMb,
                  free: memory.freeBelowMb,
                  cooldown: memory.cooldownMinutes,
                })
              : null
          }
          trailing={
            <Switch
              aria-label={t('settings.automation.memory.title')}
              checked={memory?.enabled === true}
              disabled={off}
              onCheckedChange={(enabled) => {
                if (config && memory) save({ ...config, memory: { ...memory, enabled } });
              }}
            />
          }
        />
      </ListGroup>
      {config && memory && bounds ? (
        <div className="border-t border-line py-1">
          <NumberField
            label={t('settings.automation.memory.standby')}
            unit={t('settings.automation.unit.mb')}
            value={memory.standbyAboveMb}
            range={bounds.standbyAboveMb}
            disabled={busy}
            onCommit={(standbyAboveMb) =>
              save({ ...config, memory: { ...memory, standbyAboveMb } })
            }
          />
          <NumberField
            label={t('settings.automation.memory.free')}
            unit={t('settings.automation.unit.mb')}
            value={memory.freeBelowMb}
            range={bounds.freeBelowMb}
            disabled={busy}
            onCommit={(freeBelowMb) => save({ ...config, memory: { ...memory, freeBelowMb } })}
          />
          <NumberField
            label={t('settings.automation.memory.cooldown')}
            unit={t('settings.automation.unit.minutes')}
            value={memory.cooldownMinutes}
            range={bounds.cooldownMinutes}
            disabled={busy}
            onCommit={(cooldownMinutes) =>
              save({ ...config, memory: { ...memory, cooldownMinutes } })
            }
          />
          <p className="px-4 pt-1 pb-2 text-caption text-muted">
            {t('settings.automation.memory.last', { when: when(status?.lastMemoryRun ?? null) })}
          </p>
        </div>
      ) : null}

      <ListGroup
        aria-label={t('settings.automation.clean.title')}
        className="rounded-none border-x-0 border-b-0 shadow-none"
      >
        <ListRow
          icon={<CalendarClock aria-hidden size={18} />}
          title={t('settings.automation.clean.title')}
          description={
            cleaning && daysAndTime
              ? t(
                  cleaning.frequency === 'weekly'
                    ? 'settings.automation.clean.line.weekly'
                    : 'settings.automation.clean.line.daily',
                  { day: daysAndTime.day, time: daysAndTime.time },
                )
              : null
          }
          trailing={
            <Switch
              aria-label={t('settings.automation.clean.title')}
              checked={cleaning?.enabled === true}
              disabled={off}
              onCheckedChange={(enabled) => {
                if (config && cleaning) save({ ...config, cleaning: { ...cleaning, enabled } });
              }}
            />
          }
        />
      </ListGroup>
      {config && cleaning ? (
        <div className="border-t border-line">
          <div className="flex flex-wrap items-center justify-between gap-3 px-4 py-2">
            <span className="text-body">{t('settings.automation.clean.frequency')}</span>
            <SegmentedControl
              label={t('settings.automation.clean.frequency')}
              options={[
                { value: 'daily', label: t('settings.automation.clean.daily') },
                { value: 'weekly', label: t('settings.automation.clean.weekly') },
              ]}
              value={cleaning.frequency}
              onValueChange={(frequency) => {
                if (!busy) save({ ...config, cleaning: { ...cleaning, frequency } });
              }}
            />
          </div>
          <div className="flex flex-wrap items-center justify-between gap-3 px-4 py-2">
            {cleaning.frequency === 'weekly' ? (
              <label className="flex items-center gap-2 text-body">
                {t('settings.automation.clean.day')}
                <select
                  value={cleaning.weekday}
                  disabled={busy}
                  onChange={(event) =>
                    save({
                      ...config,
                      cleaning: { ...cleaning, weekday: Number(event.target.value) },
                    })
                  }
                  className={fieldClass}
                >
                  {[0, 1, 2, 3, 4, 5, 6].map((day) => (
                    <option key={day} value={day}>
                      {weekdayName(day)}
                    </option>
                  ))}
                </select>
              </label>
            ) : (
              <span />
            )}
            <label className="flex items-center gap-2 text-body">
              {t('settings.automation.clean.time')}
              <input
                type="time"
                value={timeText(cleaning.minuteOfDay)}
                disabled={busy}
                onChange={(event) => {
                  const minuteOfDay = parseTime(event.target.value);
                  if (minuteOfDay !== null)
                    save({ ...config, cleaning: { ...cleaning, minuteOfDay } });
                }}
                className={fieldClass}
              />
            </label>
          </div>
          <h3 className="px-4 pt-2 text-body font-semibold">
            {t('settings.automation.clean.rules')}
          </h3>
          <p className="px-4 pb-2 text-caption text-muted">
            {t('settings.automation.clean.rules.hint')}
          </p>
          <ul
            aria-label={t('settings.automation.clean.rules')}
            className="divide-y divide-line border-t border-line"
          >
            {rules.map((rule) => {
              const title = tx(`rule.${rule.id}.title`, rule.title);
              const chosen = cleaning.ruleIds.includes(rule.id);
              return (
                <li key={rule.id} className="flex items-center gap-3 px-4 py-2">
                  <span className="min-w-0 flex-1 text-body">{title}</span>
                  <Badge risk={rule.risk} />
                  <Switch
                    aria-label={title}
                    checked={chosen}
                    disabled={busy}
                    onCheckedChange={(on) =>
                      save({
                        ...config,
                        cleaning: {
                          ...cleaning,
                          ruleIds: on
                            ? [...cleaning.ruleIds, rule.id]
                            : cleaning.ruleIds.filter((id) => id !== rule.id),
                        },
                      })
                    }
                  />
                </li>
              );
            })}
          </ul>
          {cleaning.enabled && cleaning.ruleIds.length === 0 ? (
            <p role="status" className="px-4 py-2 text-caption font-semibold">
              {t('settings.automation.clean.noneChosen')}
            </p>
          ) : null}
          <div className="space-y-0.5 border-t border-line px-4 py-2 text-caption text-muted">
            <p>
              {t('settings.automation.clean.last', { when: when(status?.lastCleanRun ?? null) })}
            </p>
            {cleaning.enabled && status?.nextCleanRun != null ? (
              <p>{t('settings.automation.clean.next', { when: when(status.nextCleanRun) })}</p>
            ) : null}
            {status?.lastCleanSkip ? (
              <p>
                {t('settings.automation.clean.skipped', {
                  when: when(status.lastCleanSkip.at),
                  reason: t(reasonKeys[status.lastCleanSkip.reason]),
                })}
              </p>
            ) : null}
          </div>
        </div>
      ) : null}

      <ListGroup
        aria-label={t('settings.automation.notify.title')}
        className="rounded-none border-x-0 border-b-0 shadow-none"
      >
        <ListRow
          icon={<Bell aria-hidden size={18} />}
          title={t('settings.automation.notify.title')}
          description={t('settings.automation.notify.description')}
          trailing={
            <Switch
              aria-label={t('settings.automation.notify.title')}
              checked={config?.notify === true}
              disabled={off}
              onCheckedChange={(notify) => {
                if (config) save({ ...config, notify });
              }}
            />
          }
        />
      </ListGroup>
      {error ? (
        <div role="alert" className="px-4 py-3">
          <Muted>{error}</Muted>
        </div>
      ) : null}
    </Card>
  );
}
