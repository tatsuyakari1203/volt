// The translator: typed keys, interpolation, plurals and locale-aware numbers, with no
// dependency. English is the source language and the fallback. Every other language lives in a
// JSON file next to `en.json` and must carry the same keys (`scripts/check-i18n.mjs` enforces it).
//
// Text that comes from the Rust core (tweaks, cleaning rules) stays English in the core. Other
// languages key their text by the stable id in `vi.catalogue.json`, and `tx` looks it up with the
// English from the core as the fallback. Sentences the core builds with data (health reasons,
// activity summaries) arrive as a code and parameters; `say.ts` writes them.
import en from './locales/en.json';
import viCatalogue from './locales/vi.catalogue.json';
import vi from './locales/vi.json';

/** A language the UI is written in. */
export type Locale = 'en' | 'vi';

/** What the user chose in Settings. `system` follows the language of Windows. */
export type LanguageChoice = 'system' | Locale;

/** Every key of the English file: a key that does not exist fails the typecheck. */
export type MessageKey = keyof typeof en;

/** `files` for the pair of keys `files.one` and `files.other`. */
export type PluralKey = {
  [K in MessageKey]: K extends `${infer Base}.other` ? Base : never;
}[MessageKey];

/** Values for the `{name}` placeholders of a message. Numbers are formatted for the locale. */
export type Params = Readonly<Record<string, string | number>>;

type Table = Readonly<Record<string, string>>;

const tables: Record<Locale, Table> = {
  en,
  vi: { ...vi, ...viCatalogue },
};

/** localStorage key that remembers the choice so the first paint is already in the right language. */
export const LANGUAGE_STORAGE_KEY = 'volt.language';

/** localStorage key that remembers what `System` stood for, for the first paint of the next start. */
export const SYSTEM_STORAGE_KEY = 'volt.language.system';

let current: Locale = 'en';

/** Sets the language of every `t` call that follows. The provider calls it; tests may too. */
export function setLocale(locale: Locale): void {
  current = locale;
}

/** The language `t` writes in right now. */
export function getLocale(): Locale {
  return current;
}

/** The BCP 47 tag that `Intl` and `toLocaleString` use for the current language. */
export function localeTag(): string {
  return current === 'vi' ? 'vi-VN' : 'en-US';
}

/** A number written the way the current language writes it (`12,345` or `12.345`). */
export function formatNumber(value: number, options?: Intl.NumberFormatOptions): string {
  return new Intl.NumberFormat(localeTag(), options).format(value);
}

function fill(template: string, params: Params | undefined): string {
  if (!params) return template;
  return template.replace(/\{(\w+)\}/g, (whole, name: string) => {
    const value = params[name];
    if (value === undefined) return whole;
    return typeof value === 'number' ? formatNumber(value) : value;
  });
}

/** The text of a key in the current language, or in English when the language lacks it. */
export function t(key: MessageKey, params?: Params): string {
  return fill(tables[current][key] ?? tables.en[key] ?? key, params);
}

/** The unfilled text of a key, for `tNodes` to put elements into the placeholders. */
export function rawMessage(key: MessageKey): string {
  return tables[current][key] ?? tables.en[key] ?? key;
}

/**
 * The text for a count: `key.one` or `key.other` as the language's plural rules choose, with
 * `{count}` filled in. Vietnamese has one form, so its file only needs `key.other`.
 */
export function tn(key: PluralKey, count: number, params?: Params): string {
  const category = new Intl.PluralRules(localeTag()).select(count);
  const table = tables[current];
  const template =
    table[`${key}.${category}`] ??
    table[`${key}.other`] ??
    tables.en[`${key}.${category}`] ??
    tables.en[`${key}.other`] ??
    key;
  return fill(template, { count, ...params });
}

/**
 * Text that the Rust core writes in English: the translation filed under a stable id (a tweak id,
 * a cleaning rule id, a health reason id) in the current language, or `english` from the core.
 */
export function tx(key: string, english: string): string {
  return current === 'en' ? english : (tables[current][key] ?? english);
}

/**
 * Resolves the Settings choice: `System` is `system`, the display language of Windows as the
 * shell reads it (never the browser language list).
 */
export function resolveLocale(choice: LanguageChoice, system: Locale): Locale {
  return choice === 'system' ? system : choice;
}

/** Narrows an unknown value to a language. */
export function parseLocale(value: unknown): Locale | null {
  return value === 'en' || value === 'vi' ? value : null;
}

/** Narrows an unknown value to a language choice. */
export function parseChoice(value: unknown): LanguageChoice | null {
  return value === 'system' || value === 'en' || value === 'vi' ? value : null;
}

/** Reads a `?lang=en|vi` override from a query string. Used for screenshots, never stored. */
export function readLanguageOverride(search: string): Locale | null {
  const value = new URLSearchParams(search).get('lang');
  return value === 'en' || value === 'vi' ? value : null;
}
