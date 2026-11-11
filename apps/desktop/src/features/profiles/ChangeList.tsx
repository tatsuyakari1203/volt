import { say, t } from '../../shared/i18n';
import type { TweakStepView } from '../../shared/ipc';

/** The exact changes of a tweak: one line per registry value, service or command. */
export function ChangeList({ steps, label }: { steps: TweakStepView[]; label: string }) {
  return (
    <ul aria-label={label} className="mt-1 flex flex-col gap-1.5">
      {steps.map((step) => (
        <li key={step.subject} className="rounded-sm bg-surface-sunken px-2 py-1.5">
          <code className="block break-all font-mono text-caption text-fg">
            {say(step.subjectPhrase, step.subject)}
          </code>
          <span className="block text-caption text-muted">
            {say(step.currentPhrase, step.current)} <span aria-hidden>{'->'}</span>
            <span className="sr-only">{t('tweaks.row.changesTo')}</span>{' '}
            {say(step.targetPhrase, step.target)}
          </span>
        </li>
      ))}
    </ul>
  );
}
