import { Loader2 } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { t, tn } from '../../shared/i18n';
import {
  type InspectionView,
  inspectApp,
  type PriorityView,
  type ProcessGroupView,
  type TuneRequestView,
  type TuneResult,
  tuneApp,
} from '../../shared/ipc';
import { describeError } from '../../shared/text/memory';
import { Button, Dialog } from '../../shared/ui';
import { PRIORITIES, priorityLabel, processesText } from './processes';

export interface TuneDialogProps {
  /** The app to change, or `null` while the dialog is closed. */
  group: ProcessGroupView | null;
  onClose: () => void;
  /** Called with the result once the change was made; the screen shows it and closes this. */
  onDone: (result: TuneResult) => void;
}

type EfficiencyChoice = 'keep' | 'on' | 'off';

const SELECT_CLASS =
  'h-9 w-full rounded-md border border-line bg-surface px-2 text-body text-fg focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-focus disabled:opacity-50';

/** The request the form asks for: what is left on "leave as it is" is `null`. */
export function buildRequest(form: {
  priority: PriorityView | 'keep';
  limitCores: boolean;
  cores: number[];
  efficiency: EfficiencyChoice;
}): TuneRequestView {
  return {
    // Efficiency mode sets Low priority itself, so the priority is not sent with it.
    priority: form.priority === 'keep' || form.efficiency === 'on' ? null : form.priority,
    cores: form.limitCores ? [...form.cores].sort((a, b) => a - b) : null,
    efficiency: form.efficiency === 'keep' ? null : form.efficiency === 'on',
  };
}

function nowText(value: string): string {
  return t('processes.tune.now', { value });
}

/**
 * What can be changed for one app: its priority class (never Realtime), the cores it runs on and
 * Efficiency mode, and whether to save them as a rule that is applied again whenever the app
 * starts. It first reads what the processes hold now and how many of them the user may change; the
 * rules about that live in the Rust core. The core picker never lets the last chosen core go, so
 * an empty set cannot be sent (the core refuses it too).
 */
export function TuneDialog({ group, onClose, onDone }: TuneDialogProps) {
  const [inspection, setInspection] = useState<InspectionView | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [priority, setPriority] = useState<PriorityView | 'keep'>('keep');
  const [limitCores, setLimitCores] = useState(false);
  const [cores, setCores] = useState<number[]>([]);
  const [efficiency, setEfficiency] = useState<EfficiencyChoice>('keep');
  const [saveRule, setSaveRule] = useState(false);
  const [applying, setApplying] = useState(false);
  const alive = useRef(true);

  const id = group?.id ?? null;
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  // Reads the settings each time the dialog opens for an app.
  useEffect(() => {
    setInspection(null);
    setProblem(null);
    setPriority('keep');
    setLimitCores(false);
    setEfficiency('keep');
    setSaveRule(false);
    setApplying(false);
    if (id === null) return;
    let stale = false;
    inspectApp(id)
      .then((read) => {
        if (stale) return;
        setInspection(read);
        setCores(read.cores ?? read.availableCores);
      })
      .catch((error: unknown) => {
        if (!stale) setProblem(describeError(error));
      });
    return () => {
      stale = true;
    };
  }, [id]);

  if (group === null) return <Dialog open={false} onClose={onClose} title="" />;

  const nothingToChange = inspection !== null && inspection.changeable === 0;
  const request = buildRequest({ priority, limitCores, cores, efficiency });
  const empty = request.priority === null && request.cores === null && request.efficiency === null;
  const canApply = inspection !== null && !nothingToChange && !empty && !applying;

  const toggleCore = (core: number) => {
    setCores((current) => {
      if (!current.includes(core)) return [...current, core];
      // The last core stays: an empty set is not a thing Windows accepts.
      return current.length > 1 ? current.filter((c) => c !== core) : current;
    });
  };

  const apply = async () => {
    setApplying(true);
    setProblem(null);
    try {
      const result = await tuneApp(group.id, request, saveRule);
      if (alive.current) onDone(result);
    } catch (error) {
      if (alive.current) {
        setProblem(describeError(error));
        setApplying(false);
      }
    }
  };

  const nowPriority = inspection
    ? inspection.priority
      ? priorityLabel(inspection.priority)
      : t('processes.tune.differs')
    : '';
  const nowEfficiency = inspection
    ? inspection.efficiency === null
      ? t('processes.tune.differs')
      : inspection.efficiency
        ? t('processes.tune.on')
        : t('processes.tune.off')
    : '';

  return (
    <Dialog
      open
      onClose={() => {
        if (!applying) onClose();
      }}
      title={t('processes.tune.title', { name: group.name })}
      description={t('processes.tune.description')}
      footer={
        <>
          <Button onClick={onClose} disabled={applying}>
            {t('common.cancel')}
          </Button>
          <Button
            variant="primary"
            loading={applying}
            disabled={!canApply}
            onClick={() => void apply()}
          >
            {applying ? t('processes.tune.applying') : t('processes.tune.apply')}
          </Button>
        </>
      }
    >
      <div className="flex max-h-[60vh] flex-col gap-4 overflow-y-auto">
        {inspection === null && problem === null ? (
          <p role="status" className="flex items-center gap-2 text-body text-muted">
            <Loader2 aria-hidden size={16} className="animate-spin motion-reduce:animate-none" />
            {t('processes.tune.reading')}
          </p>
        ) : null}
        {problem ? (
          <p role="alert" className="rounded-md bg-caution p-2 text-body text-caution-fg">
            {inspection === null ? `${t('processes.tune.readError')}: ${problem}` : problem}
          </p>
        ) : null}

        {inspection ? (
          <p className="text-body text-muted">
            {nothingToChange
              ? t('processes.tune.noneChangeable', { name: group.name })
              : t('processes.tune.counts', {
                  changeable: inspection.changeable,
                  processes: processesText(inspection.processes),
                })}
            {inspection.needsAdministrator > 0 && !nothingToChange
              ? ` ${t('processes.tune.needsAdmin', {
                  processes: processesText(inspection.needsAdministrator),
                })}`
              : ''}
          </p>
        ) : null}

        {inspection && !nothingToChange ? (
          <>
            <label className="flex flex-col gap-1 text-caption font-semibold text-muted">
              {t('processes.tune.priority.label')}
              <select
                className={SELECT_CLASS}
                value={efficiency === 'on' ? 'idle' : priority}
                disabled={efficiency === 'on'}
                onChange={(event) => setPriority(event.target.value as PriorityView | 'keep')}
              >
                <option value="keep">{t('processes.tune.priority.keep')}</option>
                {PRIORITIES.map((value) => (
                  <option key={value} value={value}>
                    {priorityLabel(value)}
                  </option>
                ))}
              </select>
              <span className="font-normal">
                {efficiency === 'on'
                  ? t('processes.tune.priority.byEfficiency')
                  : `${nowText(nowPriority)}. ${t('processes.tune.priority.hint')}`}
              </span>
            </label>

            <div className="flex flex-col gap-2">
              <label className="flex items-center gap-2 text-body">
                <input
                  type="checkbox"
                  checked={limitCores}
                  onChange={(event) => setLimitCores(event.target.checked)}
                />
                {t('processes.tune.cores.limit')}
              </label>
              {limitCores ? (
                <fieldset className="flex flex-col gap-2">
                  <legend className="sr-only">{t('processes.tune.cores.group')}</legend>
                  <div className="flex flex-wrap gap-2">
                    {inspection.availableCores.map((core) => (
                      <label
                        key={core}
                        className="flex h-8 min-w-12 cursor-default items-center justify-center rounded-md border border-line bg-surface px-2 text-body tabular-nums has-checked:border-accent has-checked:bg-accent-soft has-checked:text-accent-text has-focus-visible:outline-2 has-focus-visible:outline-offset-1 has-focus-visible:outline-focus"
                      >
                        <input
                          type="checkbox"
                          className="sr-only"
                          aria-label={t('processes.tune.cores.core', { number: core })}
                          checked={cores.includes(core)}
                          onChange={() => toggleCore(core)}
                        />
                        {core}
                      </label>
                    ))}
                  </div>
                  <p className="text-caption text-muted">
                    {tn('count.core', cores.length)}. {t('processes.tune.cores.hint')}
                  </p>
                </fieldset>
              ) : null}
            </div>

            <label className="flex flex-col gap-1 text-caption font-semibold text-muted">
              {t('processes.tune.efficiency.label')}
              <select
                className={SELECT_CLASS}
                value={efficiency}
                onChange={(event) => setEfficiency(event.target.value as EfficiencyChoice)}
              >
                <option value="keep">{t('processes.tune.efficiency.keep')}</option>
                <option value="on">{t('processes.tune.efficiency.on')}</option>
                <option value="off">{t('processes.tune.efficiency.off')}</option>
              </select>
              <span className="font-normal">{nowText(nowEfficiency)}</span>
            </label>

            <label className="flex flex-col gap-1 text-body">
              <span className="flex items-start gap-2">
                <input
                  type="checkbox"
                  className="mt-1"
                  checked={saveRule}
                  disabled={empty}
                  onChange={(event) => setSaveRule(event.target.checked)}
                />
                {t('processes.tune.rule.label', { name: group.name })}
              </span>
              <span className="pl-6 text-caption text-muted">{t('processes.tune.rule.hint')}</span>
            </label>
          </>
        ) : null}
      </div>
    </Dialog>
  );
}
