import { Shell } from './app/Shell';
import { ThemeProvider } from './shared/theme';
import { ToastProvider } from './shared/ui';

/** Application root: providers and the shell. Screens are mounted by the shell. */
export function App() {
  return (
    <ThemeProvider>
      <ToastProvider>
        <Shell />
      </ToastProvider>
    </ThemeProvider>
  );
}
