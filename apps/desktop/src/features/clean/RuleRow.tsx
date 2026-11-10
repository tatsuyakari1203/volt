import { Lock } from 'lucide-react';
import type { CleaningRuleInfo, RuleScanView } from '../../shared/ipc';
import { countOf } from '../../shared/text/size';
import { Badge, Switch } from '../../shared/ui';
import { findingsLine } from './messages';

interface RuleRowProps {
  rule: CleaningRuleInfo;
  /** What the scan found; `undefined` before the first scan. */
  view?: RuleScanView;
  checked: boolean;
  /** Whether the switch can be used (false while busy). */
  enabled: boolean;
  onCheckedChange: (checked: boolean) => void;
}

/**
 * One cleaning rule: its name, risk and size, a switch to include it, and a details disclosure with
 * what it takes and a few examples relative to its location. A rule that needs administrator rights
 * shows a lock label and its switch cannot be turned on: the elevation model is not decided yet.
 */
export function RuleRow({ rule, view, checked, enabled, onCheckedChange }: RuleRowProps) {
  const locked = rule.needsAdministrator;
  return (
    <li className="px-4 py-3">
      <div className="flex items-center gap-3">
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
            <span className="font-semibold">{rule.title}</span>
            <Badge risk={rule.risk} />
            {locked ? (
              <Badge>
                <Lock aria-hidden size={12} />
                Needs administrator rights
              </Badge>
            ) : null}
          </div>
          <p className="mt-0.5 text-caption text-muted">
            {view ? findingsLine(view) : 'Not scanned yet'}
            {view?.truncated ? ' (there is more than this scan lists)' : ''}
          </p>
          {view?.skipped ? (
            <p className="text-caption text-muted">Left out: {view.skipped}</p>
          ) : null}
          {view?.lockedNote ? <p className="text-caption text-muted">{view.lockedNote}</p> : null}
          {locked ? (
            <p className="text-caption text-muted">
              Volt cannot clean this yet. Nothing here is touched.
            </p>
          ) : null}
        </div>
        {view ? (
          <Switch
            aria-label={`Include ${rule.title}`}
            checked={checked && !locked}
            disabled={!enabled || locked}
            onCheckedChange={onCheckedChange}
          />
        ) : null}
      </div>
      <details className="mt-1 text-caption text-muted">
        <summary className="w-fit cursor-pointer rounded-sm py-0.5 font-semibold hover:text-fg">
          What is this?
        </summary>
        <div className="mt-1 flex flex-col gap-2">
          <p>{rule.description}</p>
          {rule.minAgeDays > 0 ? (
            <p>Only items not changed for {countOf(rule.minAgeDays, 'day')} are taken.</p>
          ) : null}
          {view && view.samples.length > 0 ? (
            <div>
              <p className="font-semibold">Examples</p>
              <ul className="font-mono wrap-anywhere">
                {view.samples.map((sample) => (
                  <li key={sample}>{sample}</li>
                ))}
              </ul>
            </div>
          ) : null}
        </div>
      </details>
    </li>
  );
}
