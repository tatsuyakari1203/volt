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
  readLanguageOverride,
  resolveLocale,
  setLocale,
} from './translate';

/** The languages the system lists, best first. Empty where there is no `navigator`. */
function systemLanguages(): readonly string[] {
  if (typeof navigator === 'undefined') return [];
  return navigator.languages?.length ? navigator.languages : [navigator.language];
}

/** The language that `System` stands for on this PC. */
function systemLocale(): Locale {
  return resolveLocale('system', systemLanguages());
}

/** Reads the remembered choice, tolerating blocked or empty storage. */
function readStoredChoice(): LanguageChoice {
  try {
    return parseChoice(getStorage()?.getItem(LANGUAGE_STORAGE_KEY)) ?? 'system';
  } catch {
    return 'system';
  }
}

function storeChoice(choice: LanguageChoice) {
  try {
    getStorage()?.setItem(LANGUAGE_STORAGE_KEY, choice);
  } catch {
    // The settings store is the source of truth; this copy only avoids a flash at start.
  }
}

/** The language for the first paint: a `?lang=` override, else the remembered choice. */
function initialLocale(): Locale {
  return (
    readLanguageOverride(window.location.search) ??
    resolveLocale(readStoredChoice(), systemLanguages())
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
 * Owns the language: follows the system by default, lets the user choose (kept in the settings
 * store of the shell, with a copy in localStorage for the first paint) and honours `?lang=en|vi`
 * for screenshots. Everything below it is translated through `t`.
 */
export function I18nProvider({ children }: { children: ReactNode }) {
  const [choice, setChoiceState] = useState<LanguageChoice>(readStoredChoice);
  const override = useMemo(() => readLanguageOverride(window.location.search), []);
  const locale = override ?? resolveLocale(choice, systemLanguages());
  // Set while rendering, so the children rendered in this pass already read the new language.
  setLocale(locale);

  useEffect(() => {
    document.documentElement.lang = locale;
  }, [locale]);

  // The settings store of the shell is the source of truth. It also learns what System means on
  // this PC, which the tray needs when no window is open.
  useEffect(() => {
    let alive = true;
    getLanguageSetting()
      .then((stored) => {
        if (!alive) return;
        setChoiceState(stored.choice);
        storeChoice(stored.choice);
        const system = systemLocale();
        if (system !== stored.system) {
          saveLanguageSetting(stored.choice, system).catch(() => {});
        }
      })
      .catch(() => {
        // Outside the Volt window there is no store: the remembered choice stands.
      });
    return () => {
      alive = false;
    };
  }, []);

  const setChoice = useCallback((next: LanguageChoice) => {
    setChoiceState(next);
    storeChoice(next);
    saveLanguageSetting(next, systemLocale()).catch(() => {});
  }, []);

  const api = useMemo(
    () => ({ choice, locale, locked: override !== null, setChoice }),
    [choice, locale, override, setChoice],
  );
  return <LanguageContext.Provider value={api}>{children}</LanguageContext.Provider>;
}
