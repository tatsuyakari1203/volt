// Sentences the Rust core builds from data. The core sends a code and parameters (a `PhraseView`)
// next to its English sentence; this writes the sentence in the language of the UI from the code,
// with no parsing of English. English is the fallback: the core's own sentence is shown in
// English, for a code this file does not know and for records stored before phrases existed.
import type { ArgView, PhraseView } from '../ipc/bindings';
import { formatBytes, formatSize } from '../text/size';
import viPhrases from './locales/vi.phrases.json';
import { formatNumber, getLocale, type Locale, localeTag } from './translate';

type Table = Readonly<Record<string, string>>;

/** The templates of each language by code. English has none: the core writes it. */
const tables: Record<Locale, Table> = { en: {}, vi: viPhrases };

/** The template of a phrase: its plural form when it counts something, else the plain one. */
function templateOf(table: Table, phrase: PhraseView): string | undefined {
  const count = phrase.args.count;
  if (count?.kind === 'count') {
    const category = new Intl.PluralRules(localeTag()).select(count.value);
    const plural = table[`${phrase.code}.${category}`] ?? table[`${phrase.code}.other`];
    if (plural !== undefined) return plural;
  }
  return table[phrase.code];
}

function argText(table: Table, arg: ArgView): string | null {
  switch (arg.kind) {
    case 'text':
      return arg.value;
    case 'count':
      return formatNumber(arg.value);
    case 'tenths':
      return formatNumber(arg.value / 10, { minimumFractionDigits: 1, maximumFractionDigits: 1 });
    case 'size':
      return formatSize(arg.value);
    case 'bytes':
      return formatBytes(arg.value);
    case 'phrase':
      return write(table, arg.value);
    case 'list': {
      const parts = arg.value.items.map((item) => write(table, item));
      return parts.every((part): part is string => part !== null)
        ? parts.join(arg.value.sep)
        : null;
    }
    default:
      return null;
  }
}

/** The phrase written from `table`, or `null` when it or a part of it has no template. */
function write(table: Table, phrase: PhraseView): string | null {
  const template = templateOf(table, phrase);
  if (template === undefined) return null;
  let known = true;
  const text = template.replace(/\{(\w+)\}/g, (_whole, name: string) => {
    const arg = phrase.args[name];
    if (arg === undefined) return '';
    const written = argText(table, arg);
    if (written === null) known = false;
    return written ?? '';
  });
  return known ? text : null;
}

/**
 * A sentence the core built from data, in the current language: written from its code and
 * parameters in Vietnamese, and as `english` (the core's own sentence) in English, when there is
 * no phrase (a record stored before phrases existed) or the UI does not know a code in it.
 */
export function say(phrase: PhraseView | null | undefined, english: string): string {
  const locale = getLocale();
  if (locale === 'en' || !phrase) return english;
  return write(tables[locale], phrase) ?? english;
}

/** `say` for a text that starts a sentence: its first letter is a capital. */
export function sayCapitalised(phrase: PhraseView | null | undefined, english: string): string {
  const text = say(phrase, english);
  const [first = ''] = text;
  return first.toLocaleUpperCase(localeTag()) + text.slice(first.length);
}
