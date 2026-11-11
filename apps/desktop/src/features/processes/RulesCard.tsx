import { X } from 'lucide-react';
import { useCallback, useEffect, useRef, useState } from 'react';
import { t } from '../../shared/i18n';
import { listProcessRules, type RuleView, removeProcessRule } from '../../shared/ipc';
import { describeError } from '../../shared/text/memory';
import { Button, Card } from '../../shared/ui';
import { ruleSummary } from './processes';

/** How often the list is read again: a rule's problem appears when the app next starts. */
const REREAD_MS = 10_000;

/**
 * Reads the saved rules when the card opens, about every ten seconds while it is open (a rule that
 * could not be applied shows its problem) and on `reload`. A failed reading keeps the list on
 * screen and reports the problem.
 */
export function useProcessRules(): {
  rules: RuleView[] | null;
  problem: string | null;
  reload: () => void;
  remove: (id: string) => Promise<void>;
  busy: boolean;
} {
  const [rules, setRules] = useState<RuleView[] | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const alive = useRef(true);

  const reload = useCallback(() => {
    listProcessRules()
      .then((list) => {
        if (alive.current) {
          setRules(list);
          setProblem(null);
        }
      })
      .catch((error: unknown) => {
        if (alive.current) setProblem(describeError(error));
      });
  }, []);

  useEffect(() => {
    alive.current = true;
    reload();
    const timer = window.setInterval(reload, REREAD_MS);
    return () => {
      alive.current = false;
      window.clearInterval(timer);
    };
  }, [reload]);

  const remove = useCallback(async (id: string) => {
    setBusy(true);
    try {
      const list = await removeProcessRule(id);
      if (alive.current) {
        setRules(list);
        setProblem(null);
      }
    } catch (error) {
      if (alive.current) setProblem(describeError(error));
    } finally {
      if (alive.current) setBusy(false);
    }
  }, []);

  return { rules, problem, reload, remove, busy };
}

export interface RulesCardProps {
  rules: RuleView[] | null;
  problem: string | null;
  busy: boolean;
  onRemove: (id: string) => void;
}

/**
 * The saved rules: each says which app it is for and what it applies, shows why it could not be
 * applied (once the core has tried), and can be removed. Adding one happens in the Settings
 * dialog of an app.
 */
export function RulesCard({ rules, problem, busy, onRemove }: RulesCardProps) {
  return (
    <Card as="section" aria-label={t('processes.rules.title')} className="flex flex-col gap-3">
      <div>
        <h2 className="text-subtitle font-semibold">{t('processes.rules.title')}</h2>
        <p className="text-body text-muted">{t('processes.rules.description')}</p>
      </div>
      {problem ? (
        <p role="alert" className="rounded-md bg-caution p-2 text-body text-caution-fg">
          {t('processes.rules.error', { problem })}
        </p>
      ) : null}
      {rules && rules.length === 0 ? (
        <p className="text-body text-muted">{t('processes.rules.empty')}</p>
      ) : null}
      {rules && rules.length > 0 ? (
        <ul aria-label={t('processes.rules.list.label')} className="flex flex-col gap-1">
          {rules.map((rule) => (
            <li
              key={rule.id}
              className="flex items-center justify-between gap-2 rounded-md border border-line px-3 py-1"
            >
              <span className="min-w-0">
                <span className="block truncate font-semibold" title={rule.path ?? rule.name}>
                  {rule.name}
                </span>
                <span className="block text-caption text-muted">{ruleSummary(rule.request)}</span>
                {rule.problem ? (
                  <span className="block text-caption text-caution-fg">
                    {rule.problem === 'needsAdministrator'
                      ? t('processes.rules.problem.admin')
                      : t('processes.rules.problem.failed')}
                  </span>
                ) : null}
              </span>
              <Button
                size="sm"
                variant="ghost"
                disabled={busy}
                aria-label={t('processes.rules.remove.label', { name: rule.name })}
                icon={<X aria-hidden size={16} />}
                onClick={() => onRemove(rule.id)}
              >
                {t('processes.rules.remove')}
              </Button>
            </li>
          ))}
        </ul>
      ) : null}
    </Card>
  );
}
