import {
  createContext,
  type ReactNode,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
} from 'react';
import {
  applyTheme,
  getStorage,
  type ResolvedTheme,
  readQueryOverride,
  readStoredPreference,
  resolveTheme,
  storePreference,
  systemPrefersDark,
  type ThemePreference,
} from './theme';

interface ThemeApi {
  /** The user's choice: system, light or dark. */
  preference: ThemePreference;
  /** The theme currently painted. */
  resolved: ResolvedTheme;
  /** True when a `?theme=` URL parameter pins the theme for this session. */
  locked: boolean;
  /** Changes and persists the preference. */
  setPreference: (preference: ThemePreference) => void;
}

const ThemeContext = createContext<ThemeApi | null>(null);

/** Returns the theme state. Must be used under a `ThemeProvider`. */
export function useTheme(): ThemeApi {
  const api = useContext(ThemeContext);
  if (!api) {
    throw new Error('useTheme must be used inside a ThemeProvider');
  }
  return api;
}

/**
 * Owns the theme: follows the system by default, lets the user override it (persisted locally),
 * and honours a `?theme=light|dark` URL parameter for screenshots.
 */
export function ThemeProvider({ children }: { children: ReactNode }) {
  const [preference, setPreferenceState] = useState<ThemePreference>(() =>
    readStoredPreference(getStorage()),
  );
  const [systemDark, setSystemDark] = useState(systemPrefersDark);
  const override = useMemo(() => readQueryOverride(window.location.search), []);

  useEffect(() => {
    if (typeof window.matchMedia !== 'function') return;
    const query = window.matchMedia('(prefers-color-scheme: dark)');
    const onChange = (event: MediaQueryListEvent) => setSystemDark(event.matches);
    query.addEventListener('change', onChange);
    return () => query.removeEventListener('change', onChange);
  }, []);

  const resolved = override ?? resolveTheme(preference, systemDark);

  useEffect(() => {
    applyTheme(resolved);
  }, [resolved]);

  const setPreference = useCallback((next: ThemePreference) => {
    setPreferenceState(next);
    storePreference(getStorage(), next);
  }, []);

  const api = useMemo(
    () => ({ preference, resolved, locked: override !== null, setPreference }),
    [preference, resolved, override, setPreference],
  );

  return <ThemeContext.Provider value={api}>{children}</ThemeContext.Provider>;
}
