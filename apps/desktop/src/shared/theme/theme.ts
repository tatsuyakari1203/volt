/** What the user chose. `system` follows the operating system. */
export type ThemePreference = 'system' | 'light' | 'dark';
/** What is actually painted. */
export type ResolvedTheme = 'light' | 'dark';

/** localStorage key holding the persisted preference. */
export const THEME_STORAGE_KEY = 'volt.theme';

/** Narrows an unknown value to a theme preference. */
export function parsePreference(value: unknown): ThemePreference | null {
  return value === 'system' || value === 'light' || value === 'dark' ? value : null;
}

/**
 * Reads a `?theme=light|dark` override from a query string. Used for screenshots and never
 * persisted. `system` is not accepted here: an override exists to pin a concrete theme.
 */
export function readQueryOverride(search: string): ResolvedTheme | null {
  const value = new URLSearchParams(search).get('theme');
  return value === 'light' || value === 'dark' ? value : null;
}

/** Reads the persisted preference, tolerating blocked or empty storage. */
export function readStoredPreference(storage: Pick<Storage, 'getItem'> | null): ThemePreference {
  try {
    return parsePreference(storage?.getItem(THEME_STORAGE_KEY)) ?? 'system';
  } catch {
    return 'system';
  }
}

/** Persists the preference. Failure (private mode, blocked storage) is harmless and ignored. */
export function storePreference(storage: Pick<Storage, 'setItem'> | null, value: ThemePreference) {
  try {
    storage?.setItem(THEME_STORAGE_KEY, value);
  } catch {
    // The theme still applies for this session.
  }
}

/** Resolves a preference against the current system setting. */
export function resolveTheme(
  preference: ThemePreference,
  systemPrefersDark: boolean,
): ResolvedTheme {
  if (preference === 'system') return systemPrefersDark ? 'dark' : 'light';
  return preference;
}

/** Whether the OS currently asks for a dark theme. False when `matchMedia` is unavailable. */
export function systemPrefersDark(): boolean {
  return typeof window.matchMedia === 'function'
    ? window.matchMedia('(prefers-color-scheme: dark)').matches
    : false;
}

/** Safe handle on localStorage: some environments throw on access. */
export function getStorage(): Storage | null {
  try {
    return window.localStorage;
  } catch {
    return null;
  }
}

/** Writes the resolved theme to the `data-theme` attribute that the CSS tokens key on. */
export function applyTheme(theme: ResolvedTheme, root: HTMLElement = document.documentElement) {
  root.dataset.theme = theme;
}

/** Computes and applies the initial theme synchronously, so the first paint is correct. */
export function initTheme(): void {
  const override = readQueryOverride(window.location.search);
  const preference = readStoredPreference(getStorage());
  applyTheme(override ?? resolveTheme(preference, systemPrefersDark()));
}
