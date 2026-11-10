import { Shell } from './app/Shell';
import { I18nProvider } from './shared/i18n';
import { ThemeProvider } from './shared/theme';
import { ToastProvider } from './shared/ui';

/** Application root: providers and the shell. Screens are mounted by the shell. */
export function App() {
  return (
    <I18nProvider>
      <ThemeProvider>
        <ToastProvider>
          <Shell />
        </ToastProvider>
      </ThemeProvider>
    </I18nProvider>
  );
}
