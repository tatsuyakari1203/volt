import { Languages, Monitor, Moon, Sun } from 'lucide-react';
import { AppInfo } from '../features/app-info/AppInfo';
import { BoostSettings } from '../features/settings/BoostSettings';
import { StartupSettings } from '../features/settings/StartupSettings';
import { type LanguageChoice, t, useLanguage } from '../shared/i18n';
import { useTheme } from '../shared/theme';
import {
  Card,
  ErrorBoundary,
  Muted,
  PageHeader,
  SegmentedControl,
  type SegmentedOption,
} from '../shared/ui';

function themeOptions(): ReadonlyArray<SegmentedOption<'system' | 'light' | 'dark'>> {
  return [
    { value: 'system', label: t('settings.theme.system'), icon: <Monitor aria-hidden size={14} /> },
    { value: 'light', label: t('settings.theme.light'), icon: <Sun aria-hidden size={14} /> },
    { value: 'dark', label: t('settings.theme.dark'), icon: <Moon aria-hidden size={14} /> },
  ];
}

function languageOptions(): ReadonlyArray<SegmentedOption<LanguageChoice>> {
  return [
    {
      value: 'system',
      label: t('settings.language.system'),
      icon: <Monitor aria-hidden size={14} />,
    },
    { value: 'en', label: t('settings.language.en') },
    { value: 'vi', label: t('settings.language.vi') },
  ];
}

/** Settings: appearance, language, Boost, startup and an About block fed by the Rust core. */
export function SettingsScreen() {
  const { preference, setPreference, locked } = useTheme();
  const language = useLanguage();
  return (
    <>
      <PageHeader title={t('settings.title')} />
      <div className="flex flex-col gap-4">
        <Card as="section" aria-labelledby="settings-appearance">
          <h2 id="settings-appearance" className="text-subtitle font-semibold">
            {t('settings.appearance')}
          </h2>
          <div className="mt-3 flex flex-wrap items-center justify-between gap-3">
            <p className="text-body">{t('settings.theme')}</p>
            <SegmentedControl
              label={t('settings.theme')}
              options={themeOptions()}
              value={preference}
              onValueChange={setPreference}
            />
          </div>
          <p className="mt-2 text-caption text-muted">
            {locked ? t('settings.theme.locked') : t('settings.theme.hint')}
          </p>
        </Card>
        <Card as="section" aria-labelledby="settings-language">
          <h2 id="settings-language" className="text-subtitle font-semibold">
            {t('settings.language')}
          </h2>
          <div className="mt-3 flex flex-wrap items-center justify-between gap-3">
            <p className="flex items-center gap-2 text-body">
              <Languages aria-hidden size={16} />
              {t('settings.language')}
            </p>
            <SegmentedControl
              label={t('settings.language')}
              options={languageOptions()}
              value={language.choice}
              onValueChange={language.setChoice}
            />
          </div>
          <p className="mt-2 text-caption text-muted">
            {language.locked ? t('settings.language.locked') : t('settings.language.hint')}
          </p>
        </Card>
        <BoostSettings />
        <StartupSettings />
        <Card as="section" aria-labelledby="settings-about">
          <h2 id="settings-about" className="text-subtitle font-semibold">
            {t('settings.about')}
          </h2>
          <div className="mt-3">
            {/* The Rust call fails outside the Tauri window (a plain browser). Contain that here. */}
            <ErrorBoundary fallback={<Muted>{t('settings.about.unavailable')}</Muted>}>
              <AppInfo />
            </ErrorBoundary>
          </div>
        </Card>
      </div>
    </>
  );
}
