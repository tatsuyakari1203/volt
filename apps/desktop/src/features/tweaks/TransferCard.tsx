import { useRef, useState } from 'react';
import { say, t, tn, tx } from '../../shared/i18n';
import {
  applyConfigurationImport,
  exportConfiguration,
  type ImportPreview,
  previewConfigurationImport,
  type SettingImportView,
  type TweakImportView,
} from '../../shared/ipc';
import { Button, Card, Dialog, useToast } from '../../shared/ui';
import { describeError } from './messages';
import { StepList } from './TweakRow';

type Working = 'export' | 'read' | 'import';

interface Waiting {
  fileName: string;
  preview: ImportPreview;
}

export interface TransferCardProps {
  /** Reports something the user has to look at (a persistent notice on the page). */
  onProblem: (message: string) => void;
  /** Called after an import changed something, so the page reads the system again. */
  onImported: () => void;
}

/** What the file calls a tweak: its name here, in the current language, or the id it holds. */
function nameOf(item: { id: string; title: string | null }): string {
  return item.title === null ? item.id : tx(`tweak.${item.id}.title`, item.title);
}

const SETTING_LABELS = {
  'boost.config': 'tweaks.transfer.setting.boost.config',
  'memory.exclusions': 'tweaks.transfer.setting.memory.exclusions',
  'profiles.list': 'tweaks.transfer.setting.profiles.list',
} as const;

function settingName(key: string): string {
  return key in SETTING_LABELS ? t(SETTING_LABELS[key as keyof typeof SETTING_LABELS]) : key;
}

function skipReason(item: TweakImportView): string {
  if (item.action.kind !== 'skip') return '';
  switch (item.action.reason) {
    case 'unknown':
      return t('tweaks.transfer.skip.unknown');
    case 'needsAdministrator':
      return t('tweaks.transfer.skip.needsAdmin');
    default:
      return t('tweaks.transfer.skip.unavailable', { detail: item.action.detail ?? '' }).trim();
  }
}

function settingLine(setting: SettingImportView): string {
  switch (setting.status) {
    case 'same':
      return t('tweaks.transfer.setting.same');
    case 'invalid':
      return t('tweaks.transfer.setting.invalid');
    default:
      return say(setting.newPhrase, setting.new ?? '');
  }
}

/** How many things an import would change: tweaks to turn on and settings to store. */
function changeCount(preview: ImportPreview): number {
  return (
    preview.tweaks.filter((item) => item.action.kind === 'apply').length +
    preview.settings.filter((setting) => setting.status === 'change').length
  );
}

/**
 * Export and import of the configuration: the tweaks Volt turned on and a few settings, as one
 * file. The file dialogs are opened by the Rust side. An import is previewed first (nothing
 * changes), and applied only when the user confirms the exact list.
 */
export function TransferCard({ onProblem, onImported }: TransferCardProps) {
  const toast = useToast();
  const [working, setWorking] = useState<Working | null>(null);
  const [waiting, setWaiting] = useState<Waiting | null>(null);
  const cancelRef = useRef<HTMLButtonElement>(null);

  const save = async () => {
    setWorking('export');
    try {
      const result = await exportConfiguration();
      if (result.kind === 'saved') {
        toast.show({
          message: t('tweaks.transfer.exported', {
            path: result.path,
            tweaks: result.tweaks,
            settings: result.settings,
          }),
        });
      }
    } catch (error) {
      onProblem(t('tweaks.transfer.fail.export', { error: describeError(error) }));
    } finally {
      setWorking(null);
    }
  };

  const read = async () => {
    setWorking('read');
    try {
      const result = await previewConfigurationImport();
      if (result.kind === 'preview') {
        setWaiting({ fileName: result.fileName, preview: result.preview });
      }
    } catch (error) {
      onProblem(t('tweaks.transfer.fail.import', { error: describeError(error) }));
    } finally {
      setWorking(null);
    }
  };

  const importNow = async () => {
    setWaiting(null);
    setWorking('import');
    try {
      const result = await applyConfigurationImport();
      const applied = result.tweaks.filter((item) => item.status === 'applied').length;
      const failed = result.tweaks.filter(
        (item) =>
          item.status === 'failed' ||
          item.status === 'rolledBack' ||
          item.status === 'needsAttention',
      ).length;
      const params = { applied, failed, settings: result.settingsStored };
      if (failed > 0 || result.settingsFailed > 0) {
        onProblem(t('tweaks.transfer.problems', params));
      } else {
        toast.show({ message: t('tweaks.transfer.done', params) });
      }
      onImported();
    } catch (error) {
      onProblem(t('tweaks.transfer.fail.apply', { error: describeError(error) }));
    } finally {
      setWorking(null);
    }
  };

  const preview = waiting?.preview;
  const toApply = preview?.tweaks.filter((item) => item.action.kind === 'apply') ?? [];
  const already = preview?.tweaks.filter((item) => item.action.kind === 'alreadyApplied') ?? [];
  const skipped = preview?.tweaks.filter((item) => item.action.kind === 'skip') ?? [];
  const nothing = preview ? changeCount(preview) === 0 : true;

  return (
    <>
      <Card as="section" aria-label={t('tweaks.transfer.title')} className="flex flex-col gap-3">
        <div>
          <h2 className="font-semibold">{t('tweaks.transfer.title')}</h2>
          <p className="text-body text-muted">{t('tweaks.transfer.description')}</p>
        </div>
        <div className="flex flex-wrap gap-2">
          <Button
            loading={working === 'export'}
            disabled={working !== null}
            onClick={() => void save()}
          >
            {t('tweaks.transfer.export')}
          </Button>
          <Button
            loading={working === 'read'}
            disabled={working !== null}
            onClick={() => void read()}
          >
            {t('tweaks.transfer.import')}
          </Button>
        </div>
        <p className="text-caption text-muted">{t('tweaks.transfer.leftOut')}</p>
      </Card>
      <Dialog
        open={waiting !== null}
        onClose={() => setWaiting(null)}
        title={t('tweaks.transfer.confirm.title')}
        description={
          waiting ? t('tweaks.transfer.confirm.description', { file: waiting.fileName }) : undefined
        }
        initialFocusRef={cancelRef}
        footer={
          <>
            <Button ref={cancelRef} onClick={() => setWaiting(null)}>
              {t('common.cancel')}
            </Button>
            <Button variant="primary" disabled={nothing} onClick={() => void importNow()}>
              {t('tweaks.transfer.confirm.button')}
            </Button>
          </>
        }
      >
        {preview ? (
          <div className="flex max-h-80 flex-col gap-3 overflow-y-auto text-body">
            {nothing ? (
              <p className="text-muted">{t('tweaks.transfer.confirm.nothing')}</p>
            ) : (
              <p className="text-caption text-muted">{t('tweaks.transfer.confirm.undo')}</p>
            )}
            {toApply.length > 0 ? (
              <section aria-label={t('tweaks.transfer.section.apply')}>
                <h3 className="font-semibold">{t('tweaks.transfer.section.apply')}</h3>
                {toApply.map((item) => (
                  <div key={item.id} className="mt-1.5">
                    <p>{nameOf(item)}</p>
                    {item.action.kind === 'apply' ? (
                      <StepList
                        label={t('tweaks.transfer.steps', { title: nameOf(item) })}
                        steps={item.action.steps}
                      />
                    ) : null}
                  </div>
                ))}
              </section>
            ) : null}
            {preview.settings.length > 0 ? (
              <section aria-label={t('tweaks.transfer.section.settings')}>
                <h3 className="font-semibold">{t('tweaks.transfer.section.settings')}</h3>
                <ul className="mt-1 flex flex-col gap-1.5">
                  {preview.settings.map((setting) => (
                    <li key={setting.key} className="rounded-sm bg-surface-sunken px-2 py-1.5">
                      <span className="block font-semibold">{settingName(setting.key)}</span>
                      <span className="block text-caption text-muted">{settingLine(setting)}</span>
                      {setting.status === 'change' && setting.current ? (
                        <span className="block text-caption text-muted">
                          {t('tweaks.transfer.setting.change', {
                            current: say(setting.currentPhrase, setting.current),
                          })}
                        </span>
                      ) : null}
                    </li>
                  ))}
                </ul>
              </section>
            ) : null}
            {already.length > 0 ? (
              <section aria-label={t('tweaks.transfer.section.already')}>
                <h3 className="font-semibold">{t('tweaks.transfer.section.already')}</h3>
                <p className="text-muted">{already.map(nameOf).join(', ')}</p>
              </section>
            ) : null}
            {skipped.length > 0 ? (
              <section aria-label={t('tweaks.transfer.section.skipped')}>
                <h3 className="font-semibold">{t('tweaks.transfer.section.skipped')}</h3>
                <ul className="mt-1 flex flex-col gap-1.5">
                  {skipped.map((item) => (
                    <li key={item.id} className="rounded-sm bg-surface-sunken px-2 py-1.5">
                      <span className="block font-semibold">{nameOf(item)}</span>
                      <span className="block text-caption text-muted">{skipReason(item)}</span>
                    </li>
                  ))}
                </ul>
              </section>
            ) : null}
            {preview.ignoredSettings > 0 ? (
              <p className="text-caption text-muted">
                {tn('tweaks.transfer.ignored', preview.ignoredSettings)}
              </p>
            ) : null}
          </div>
        ) : null}
      </Dialog>
    </>
  );
}
