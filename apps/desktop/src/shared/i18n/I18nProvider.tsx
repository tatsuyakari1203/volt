import {
  createContext,
  type ReactNode,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
} from 'react';
import { getLanguageSetting, saveLanguageSetting } from '../ipc';
import { getStorage } from '../theme/theme';
import {
  LANGUAGE_STORAGE_KEY,
  type LanguageChoice,
  type Locale,
  parseChoice,
  parseLocale,
  readLanguageOverride,
  resolveLocale,
  SYSTEM_STORAGE_KEY,
  setLocale,
} from './translate';

/** Reads a remembered value, tolerating blocked or empty storage. */
function readStored(key: string): string | null {
  try {
    return getStorage()?.getItem(key) ?? null;
  } catch {
    return null;
  }
}

function store(key: string, value: string) {
  try {
    getStorage()?.setItem(key, value);
  } catch {
    // The settings store is the source of truth; this copy only avoids a flash at start.
  }
}

/** Reads the remembered choice. */
function readStoredChoice(): LanguageChoice {
  return parseChoice(readStored(LANGUAGE_STORAGE_KEY)) ?? 'system';
}

/**
 * What `System` stood for the last time. The shell reads the display language of Windows, so this
 * copy only serves the first paint, until the shell answers. English until it was ever known.
 */
function readStoredSystem(): Locale {
  return parseLocale(readStored(SYSTEM_STORAGE_KEY)) ?? 'en';
}

/** The language for the first paint: a `?lang=` override, else the remembered choice. */
function initialLocale(): Locale {
  return (
    readLanguageOverride(window.location.search) ??
    resolveLocale(readStoredChoice(), readStoredSystem())
  );
}

/** Sets the language before the first render, so no text flashes in the wrong one. */
export function initLanguage(): void {
  const locale = initialLocale();
  setLocale(locale);
  document.documentElement.lang = locale;
}

interface LanguageApi {
  /** What the user chose: System, English or Vietnamese. */
  choice: LanguageChoice;
  /** The language painted now. */
  locale: Locale;
  /** True when a `?lang=` parameter pins the language for this session. */
  locked: boolean;
  /** Changes the language at once and saves it. */
  setChoice: (choice: LanguageChoice) => void;
}

// Without a provider (a unit test of one component) the language is English and cannot change.
const LanguageContext = createContext<LanguageApi>({
  choice: 'system',
  locale: 'en',
  locked: false,
  setChoice: () => {},
});

/** The language state. Components that show text call it so they draw again when it changes. */
export function useLanguage(): LanguageApi {
  return useContext(LanguageContext);
}

/**
 * Owns the language: follows the display language of Windows by default (the shell reads it, the
 * browser language list is never used), lets the user choose (kept in the settings store of the
 * shell, with a copy in localStorage for the first paint) and honours `?lang=en|vi` for
 * screenshots. Everything below it is translated through `t`.
 */
export function I18nProvider({ children }: { children: ReactNode }) {
  const [choice, setChoiceState] = useState<LanguageChoice>(readStoredChoice);
  const [system, setSystem] = useState<Locale>(readStoredSystem);
  const override = useMemo(() => readLanguageOverride(window.location.search), []);
  const locale = override ?? resolveLocale(choice, system);
  // Set while rendering, so the children rendered in this pass already read the new language.
  setLocale(locale);

  useEffect(() => {
    document.documentElement.lang = locale;
  }, [locale]);

  // The settings store of the shell is the source of truth for the choice, and the shell says
  // what System stands for on this PC.
  useEffect(() => {
    let alive = true;
    getLanguageSetting()
      .then((stored) => {
        if (!alive) return;
        setChoiceState(stored.choice);
        setSystem(stored.system);
        store(LANGUAGE_STORAGE_KEY, stored.choice);
        store(SYSTEM_STORAGE_KEY, stored.system);
      })
      .catch(() => {
        // Outside the Volt window there is no shell: the remembered values stand.
      });
    return () => {
      alive = false;
    };
  }, []);

  const setChoice = useCallback((next: LanguageChoice) => {
    setChoiceState(next);
    store(LANGUAGE_STORAGE_KEY, next);
    saveLanguageSetting(next)
      .then((saved) => {
        setSystem(saved.system);
        store(SYSTEM_STORAGE_KEY, saved.system);
      })
      .catch(() => {});
  }, []);

  const api = useMemo(
    () => ({ choice, locale, locked: override !== null, setChoice }),
    [choice, locale, override, setChoice],
  );
  return <LanguageContext.Provider value={api}>{children}</LanguageContext.Provider>;
}
