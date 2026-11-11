import { I18nProvider } from '../../shared/i18n';
import { ThemeProvider } from '../../shared/theme';
import { TrayPanel } from './TrayPanel';

/** Which page this window shows: the tray panel is the main page with `?view=tray-panel`. */
export function isTrayPanel(search: string): boolean {
  return new URLSearchParams(search).get('view') === 'tray-panel';
}

/** Root of the tray panel window: the same providers as the app, and no shell around it. */
export function PanelApp() {
  return (
    <I18nProvider>
      <ThemeProvider>
        <TrayPanel />
      </ThemeProvider>
    </I18nProvider>
  );
}
