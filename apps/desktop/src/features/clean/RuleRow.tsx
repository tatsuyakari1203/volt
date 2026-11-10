import { Lock } from 'lucide-react';
import { say, t, tn } from '../../shared/i18n';
import type { CleaningRuleInfo, RuleScanView } from '../../shared/ipc';
import { formatSize } from '../../shared/text/size';
import { Badge, Button, Switch } from '../../shared/ui';
import { findingsLine, ruleDescription, ruleTitle } from './messages';

interface RuleRowProps {
  rule: CleaningRuleInfo;
  /** What the scan found; `undefined` before the first scan. */
  view?: RuleScanView;
  checked: boolean;
  /** Whether the switch can be used (false while busy). */
  enabled: boolean;
  onCheckedChange: (checked: boolean) => void;
  /** For a rule picked item by item: the positions of the items that are ticked. */
  picked?: ReadonlySet<number>;
  onPickItem?: (index: number, on: boolean) => void;
  /** For a rule that works in a folder the user adds: choose, change or remove it. */
  onChooseFolder?: () => void;
  onForgetFolder?: () => void;
}

/** The folder a rule works in, with the buttons to add, change or remove it. */
function FolderLine({
  rule,
  enabled,
  onChooseFolder,
  onForgetFolder,
}: Pick<RuleRowProps, 'rule' | 'enabled' | 'onChooseFolder' | 'onForgetFolder'>) {
  const title = ruleTitle(rule.id, rule.title);
  return (
    <div className="mt-2 flex flex-wrap items-center gap-2 text-caption">
      {rule.chosenFolder !== null ? (
        <p className="min-w-0 flex-1 basis-48 font-mono wrap-anywhere text-muted">
          <span className="font-sans font-semibold">{t('clean.folder.label')}: </span>
          {rule.chosenFolder}
        </p>
      ) : null}
      <Button
        size="sm"
        disabled={!enabled}
        aria-label={t(
          rule.chosenFolder === null ? 'clean.folder.addFor' : 'clean.folder.changeFor',
          { title },
        )}
        onClick={onChooseFolder}
      >
        {t(rule.chosenFolder === null ? 'clean.folder.add' : 'clean.folder.change')}
      </Button>
      {rule.chosenFolder !== null ? (
        <Button
          size="sm"
          disabled={!enabled}
          aria-label={t('clean.folder.removeFor', { title })}
          onClick={onForgetFolder}
        >
          {t('clean.folder.remove')}
        </Button>
      ) : null}
    </div>
  );
}

/** The items of a rule picked item by item, biggest first, each with its own tick. */
function ItemList({
  rule,
  view,
  picked,
  enabled,
  onPickItem,
}: {
  rule: CleaningRuleInfo;
  view: RuleScanView;
  picked: ReadonlySet<number>;
  enabled: boolean;
  onPickItem?: (index: number, on: boolean) => void;
}) {
  const items = [...view.items].sort((a, b) => b.bytes - a.bytes);
  return (
    <ul
      aria-label={t('clean.items.label', { title: ruleTitle(rule.id, rule.title) })}
      className="mt-2 flex flex-col divide-y divide-line rounded-md border border-line"
    >
      {items.map((item) => (
        <li key={item.index} className="flex items-center gap-3 px-3 py-2">
          <input
            type="checkbox"
            className="size-4 shrink-0 accent-(--color-accent)"
            aria-label={t('clean.items.take', { name: item.name })}
            checked={picked.has(item.index)}
            disabled={!enabled}
            onChange={(event) => onPickItem?.(item.index, event.target.checked)}
          />
          <div className="min-w-0 flex-1">
            <p className="font-semibold wrap-anywhere">{item.name}</p>
            <p className="text-caption text-muted">
              {t('clean.items.detail', {
                size: formatSize(item.bytes),
                files: tn('count.file', item.files),
              })}
            </p>
          </div>
        </li>
      ))}
    </ul>
  );
}

/**
 * One cleaning rule: its name, risk and size, a switch to include it, and a details disclosure with
 * what it takes and a few examples relative to its location. A rule that needs administrator rights
 * shows a lock label and its switch cannot be turned on: the elevation model is not decided yet.
 * A rule picked item by item has no switch: it lists its items, each with its own tick, none
 * ticked for you. A rule that works in a folder you add shows that folder and can change or
 * remove it.
 */
export function RuleRow({
  rule,
  view,
  checked,
  enabled,
  onCheckedChange,
  picked,
  onPickItem,
  onChooseFolder,
  onForgetFolder,
}: RuleRowProps) {
  const locked = rule.needsAdministrator;
  const waitingForFolder = rule.needsFolder && rule.chosenFolder === null;
  const showSwitch = view && !rule.pickItems && !waitingForFolder;
  return (
    <li className="px-4 py-3">
      <div className="flex items-center gap-3">
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
            <span className="font-semibold">{ruleTitle(rule.id, rule.title)}</span>
            <Badge risk={rule.risk} />
            {locked ? (
              <Badge>
                <Lock aria-hidden size={12} />
                {t('common.needsAdmin')}
              </Badge>
            ) : null}
          </div>
          <p className="mt-0.5 text-caption text-muted">
            {view ? findingsLine(view) : t('clean.rule.notScanned')}
            {view?.truncated ? ` ${t('clean.rule.truncated')}` : ''}
          </p>
          {view?.followedBy ? (
            <p className="text-caption text-muted">
              {t('clean.rule.followedBy', { variable: view.followedBy })}
            </p>
          ) : null}
          {view?.skipped ? (
            <p className="text-caption text-muted">
              {t('clean.rule.leftOut', { reason: say(view.skippedPhrase, view.skipped) })}
            </p>
          ) : null}
          {view?.lockedNote ? (
            <p className="text-caption text-muted">{say(view.lockedPhrase, view.lockedNote)}</p>
          ) : null}
          {locked ? <p className="text-caption text-muted">{t('clean.rule.cannotClean')}</p> : null}
        </div>
        {showSwitch ? (
          <Switch
            aria-label={t('clean.rule.include', { title: ruleTitle(rule.id, rule.title) })}
            checked={checked && !locked}
            disabled={!enabled || locked}
            onCheckedChange={onCheckedChange}
          />
        ) : null}
      </div>
      {rule.needsFolder ? (
        <FolderLine
          rule={rule}
          enabled={enabled}
          onChooseFolder={onChooseFolder}
          onForgetFolder={onForgetFolder}
        />
      ) : null}
      {view && rule.pickItems && view.items.length > 0 ? (
        <ItemList
          rule={rule}
          view={view}
          picked={picked ?? new Set()}
          enabled={enabled}
          onPickItem={onPickItem}
        />
      ) : null}
      <details className="mt-1 text-caption text-muted">
        <summary className="w-fit cursor-pointer rounded-sm py-0.5 font-semibold hover:text-fg">
          {t('clean.rule.what')}
        </summary>
        <div className="mt-1 flex flex-col gap-2">
          <p>{ruleDescription(rule)}</p>
          {rule.minAgeDays > 0 ? (
            <p>{t('clean.rule.minAge', { days: tn('count.day', rule.minAgeDays) })}</p>
          ) : null}
          {rule.follows.length > 0 ? (
            <div>
              <p className="font-semibold">{t('clean.rule.follows')}</p>
              <ul>
                {rule.follows.map((follow) => (
                  <li key={follow.var} className="wrap-anywhere">
                    {t('clean.rule.followEntry', { variable: follow.var, source: follow.source })}
                  </li>
                ))}
              </ul>
            </div>
          ) : null}
          {view && view.samples.length > 0 ? (
            <div>
              <p className="font-semibold">{t('clean.rule.examples')}</p>
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
