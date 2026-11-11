import { useEffect, useRef, useState } from 'react';
import { say, sayCapitalised, t, tn } from '../../shared/i18n';
import {
  type AutomationChangeView,
  previewProfileSwitch,
  type SwitchPreviewView,
  type SwitchResultView,
  switchProfile,
} from '../../shared/ipc';
import { Badge, Button, Dialog } from '../../shared/ui';
import { ChangeList } from './ChangeList';
import { describeError, leftOutText, stepText, tweakName } from './messages';

/** The profile to switch to (`id` is `null` to turn the active profile off) and what to call it. */
export interface SwitchTarget {
  id: string | null;
  name: string;
}

type Phase =
  | { kind: 'loading' }
  | { kind: 'error'; message: string }
  | { kind: 'preview'; preview: SwitchPreviewView }
  | { kind: 'working' }
  | { kind: 'result'; result: SwitchResultView };

export interface SwitchDialogProps {
  target: SwitchTarget;
  onClose: () => void;
  /** Called once the switch ran (whole or part), so the page reads the profiles again. */
  onSwitched: () => void;
}

function stateWord(on: boolean): string {
  return t(on ? 'profiles.state.on' : 'profiles.state.off');
}

/** The automation lines a switch would change. */
function automationLines(change: AutomationChangeView): string[] {
  const lines: string[] = [];
  if (change.before.memory.enabled !== change.after.memory.enabled) {
    lines.push(
      t('profiles.switch.automation.memory', {
        from: stateWord(change.before.memory.enabled),
        to: stateWord(change.after.memory.enabled),
      }),
    );
  }
  if (change.before.cleaning.enabled !== change.after.cleaning.enabled) {
    lines.push(
      t('profiles.switch.automation.cleaning', {
        from: stateWord(change.before.cleaning.enabled),
        to: stateWord(change.after.cleaning.enabled),
      }),
    );
  }
  if (lines.length === 0) lines.push(t('profiles.switch.automation.numbers'));
  return lines;
}

/**
 * Previews a profile switch exactly (every tweak it turns on or off with its changes, the ones it
 * leaves alone and why, the automation settings), names the Caution and Advanced tweaks, and only
 * then switches. A switch that stops part-way says what was and was not changed.
 */
export function SwitchDialog({ target, onClose, onSwitched }: SwitchDialogProps) {
  const [phase, setPhase] = useState<Phase>({ kind: 'loading' });
  const cancelRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    let current = true;
    previewProfileSwitch(target.id)
      .then((preview) => {
        if (current) setPhase({ kind: 'preview', preview });
      })
      .catch((error: unknown) => {
        if (current) setPhase({ kind: 'error', message: describeError(error) });
      });
    return () => {
      current = false;
    };
  }, [target.id]);

  async function confirm() {
    setPhase({ kind: 'working' });
    try {
      const result = await switchProfile(target.id);
      setPhase({ kind: 'result', result });
    } catch (error) {
      setPhase({ kind: 'error', message: describeError(error) });
    }
    onSwitched();
  }

  const turnOff = target.id === null;
  const done = phase.kind === 'result';
  return (
    <Dialog
      open
      onClose={phase.kind === 'working' ? () => {} : onClose}
      title={t(turnOff ? 'profiles.switch.titleOff' : 'profiles.switch.title', {
        name: target.name,
      })}
      description={done ? undefined : t('profiles.switch.journal')}
      initialFocusRef={cancelRef}
      footer={
        done || phase.kind === 'error' ? (
          <Button ref={cancelRef} variant="primary" onClick={onClose}>
            {t('profiles.close')}
          </Button>
        ) : (
          <>
            <Button ref={cancelRef} disabled={phase.kind === 'working'} onClick={onClose}>
              {t('common.cancel')}
            </Button>
            <Button
              variant="primary"
              loading={phase.kind === 'working'}
              disabled={phase.kind === 'loading'}
              onClick={() => void confirm()}
            >
              {t(turnOff ? 'profiles.switch.confirmOff' : 'profiles.switch.confirm', {
                name: target.name,
              })}
            </Button>
          </>
        )
      }
    >
      {phase.kind === 'loading' ? (
        <p role="status" className="text-body text-muted">
          {t('profiles.switch.reading')}
        </p>
      ) : null}
      {phase.kind === 'working' ? (
        <p role="status" className="text-body text-muted">
          {t('profiles.switch.working')}
        </p>
      ) : null}
      {phase.kind === 'error' ? (
        <p role="alert" className="text-body text-muted">
          {t('profiles.switch.failed', { message: phase.message })}
        </p>
      ) : null}
      {phase.kind === 'preview' ? <Preview preview={phase.preview} /> : null}
      {phase.kind === 'result' ? <Result result={phase.result} target={target} /> : null}
    </Dialog>
  );
}

function Preview({ preview }: { preview: SwitchPreviewView }) {
  const off = preview.items.filter((item) => item.action === 'turnOff');
  const on = preview.items.filter((item) => item.action === 'turnOn');
  const risky = preview.items.filter((item) => item.risk !== 'safe');
  const nothing = preview.items.length === 0 && preview.automation === null;
  return (
    <div className="flex max-h-[55vh] flex-col gap-3 overflow-y-auto pr-1">
      {nothing ? <p className="text-body">{t('profiles.switch.nothing')}</p> : null}
      {preview.inPlace > 0 ? (
        <p className="text-caption text-muted">{tn('profiles.switch.inPlace', preview.inPlace)}</p>
      ) : null}
      {risky.length > 0 ? (
        <p role="note" className="rounded-sm bg-caution px-2 py-1.5 text-body text-caution-fg">
          {t('profiles.switch.caution', {
            names: risky.map((item) => tweakName(item.tweak, item.title)).join(', '),
          })}
        </p>
      ) : null}
      {off.length > 0 ? (
        <section aria-label={t('profiles.switch.turnsOff')}>
          <h3 className="font-semibold">{t('profiles.switch.turnsOff')}</h3>
          <ul className="mt-1 flex flex-col gap-2">
            {off.map((item) => (
              <li key={item.tweak} className="text-body">
                <span className="flex flex-wrap items-center gap-2 font-semibold">
                  {tweakName(item.tweak, item.title)}
                  {item.risk !== 'safe' ? <Badge risk={item.risk} /> : null}
                </span>
                {item.restores.map((restore) => (
                  <span key={restore.entry} className="block text-caption text-muted">
                    {sayCapitalised(restore.whatPhrase, restore.what)}:{' '}
                    {t('undo.restore.will', {
                      restore: say(restore.restorePhrase, restore.restore),
                    })}
                  </span>
                ))}
              </li>
            ))}
          </ul>
        </section>
      ) : null}
      {on.length > 0 ? (
        <section aria-label={t('profiles.switch.turnsOn')}>
          <h3 className="font-semibold">{t('profiles.switch.turnsOn')}</h3>
          <ul className="mt-1 flex flex-col gap-2">
            {on.map((item) => {
              const name = tweakName(item.tweak, item.title);
              return (
                <li key={item.tweak} className="text-body">
                  <span className="flex flex-wrap items-center gap-2 font-semibold">
                    {name}
                    {item.risk !== 'safe' ? <Badge risk={item.risk} /> : null}
                  </span>
                  <ChangeList
                    label={t('tweaks.drift.changes', { title: name })}
                    steps={item.steps}
                  />
                </li>
              );
            })}
          </ul>
        </section>
      ) : null}
      {preview.automation ? (
        <section aria-label={t('profiles.switch.automation.title')}>
          <h3 className="font-semibold">{t('profiles.switch.automation.title')}</h3>
          <ul className="mt-1 text-body">
            {automationLines(preview.automation).map((line) => (
              <li key={line}>{line}</li>
            ))}
          </ul>
        </section>
      ) : null}
      {preview.leftOut.length > 0 ? (
        <section aria-label={t('profiles.switch.leftOut')}>
          <h3 className="font-semibold">{t('profiles.switch.leftOut')}</h3>
          <ul className="mt-1 text-body">
            {preview.leftOut.map((item) => (
              <li key={item.tweak}>
                <span className="font-semibold">
                  {item.title === null ? item.tweak : tweakName(item.tweak, item.title)}
                </span>
                : <span className="text-muted">{leftOutText(item)}</span>
              </li>
            ))}
          </ul>
        </section>
      ) : null}
    </div>
  );
}

function Result({ result, target }: { result: SwitchResultView; target: SwitchTarget }) {
  const changed = result.steps.filter((step) => step.status === 'done').length;
  return (
    <div className="flex max-h-[55vh] flex-col gap-3 overflow-y-auto pr-1">
      <p role={result.complete ? 'status' : 'alert'} className="text-body">
        {result.complete
          ? t(target.id === null ? 'profiles.result.doneOff' : 'profiles.result.done', {
              name: target.name,
              changes: tn('profiles.result.changes', changed),
            })
          : t('profiles.result.stopped', {
              name: target.name,
              done: changed,
              total: result.steps.length,
            })}
      </p>
      {result.steps.length > 0 ? (
        <ul aria-label={t('profiles.result.steps')} className="flex flex-col gap-1.5">
          {result.steps.map((step) => (
            <li key={step.tweak} className="text-body">
              <span className="font-semibold">{tweakName(step.tweak, step.title)}</span>
              {': '}
              <span className="text-muted">{stepText(step)}</span>
            </li>
          ))}
        </ul>
      ) : null}
      {result.automation === 'set' ? (
        <p className="text-body">{t('profiles.result.automation.set')}</p>
      ) : null}
      {result.automation === 'failed' ? (
        <p className="text-body">{t('profiles.result.automation.failed')}</p>
      ) : null}
      {result.automation === 'notAttempted' ? (
        <p className="text-body">{t('profiles.result.automation.notAttempted')}</p>
      ) : null}
      <p className="text-caption text-muted">{t('profiles.result.undoHint')}</p>
    </div>
  );
}
