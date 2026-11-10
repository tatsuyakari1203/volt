import { Monitor, Moon, Sun } from 'lucide-react';
import { AppInfo } from '../features/app-info/AppInfo';
import { useTheme } from '../shared/theme';
import {
  Card,
  ErrorBoundary,
  Muted,
  PageHeader,
  SegmentedControl,
  type SegmentedOption,
} from '../shared/ui';

const themeOptions: ReadonlyArray<SegmentedOption<'system' | 'light' | 'dark'>> = [
  { value: 'system', label: 'System', icon: <Monitor aria-hidden size={14} /> },
  { value: 'light', label: 'Light', icon: <Sun aria-hidden size={14} /> },
  { value: 'dark', label: 'Dark', icon: <Moon aria-hidden size={14} /> },
];

/** Settings: appearance, plus an About block fed by the Rust core. */
export function SettingsScreen() {
  const { preference, setPreference, locked } = useTheme();
  return (
    <>
      <PageHeader title="Settings" />
      <div className="flex flex-col gap-4">
        <Card as="section" aria-labelledby="settings-appearance">
          <h2 id="settings-appearance" className="text-subtitle font-semibold">
            Appearance
          </h2>
          <div className="mt-3 flex flex-wrap items-center justify-between gap-3">
            <p className="text-body">Theme</p>
            <SegmentedControl
              label="Theme"
              options={themeOptions}
              value={preference}
              onValueChange={setPreference}
            />
          </div>
          <p className="mt-2 text-caption text-muted">
            {locked
              ? 'The theme is pinned by a ?theme= parameter in the address for this session.'
              : 'System follows your Windows setting.'}
          </p>
        </Card>
        <Card as="section" aria-labelledby="settings-about">
          <h2 id="settings-about" className="text-subtitle font-semibold">
            About
          </h2>
          <div className="mt-3">
            {/* The Rust call fails outside the Tauri window (a plain browser). Contain that here. */}
            <ErrorBoundary fallback={<Muted>Version information is unavailable.</Muted>}>
              <AppInfo />
            </ErrorBoundary>
          </div>
        </Card>
      </div>
    </>
  );
}
