#!/usr/bin/env node
// Checks that every language of the UI is complete.
//
// The UI strings live in apps/desktop/src/shared/i18n/locales: `en.json` (the source) and one file
// per other language (`vi.json`). Text that the Rust core writes in English (tweaks and cleaning
// rules, from catalogue/*.toml) is translated by its stable id in `vi.catalogue.json`. Sentences the
// core builds with data (health reasons, activity summaries, scan notes) are phrases: a code and
// parameters, written in English by the table `ENGLISH` in crates/volt-core/src/phrase.rs and in
// Vietnamese by `vi.phrases.json`. This fails on:
//
//   - a key that exists in one language and not in the other, in either direction (a plural key
//     such as `files.one`/`files.other` counts once; a language without the `one` form is fine
//     as long as it has `other`),
//   - a `{placeholder}` that one language has and the other does not,
//   - a tweak id or a cleaning rule id (or a cleaning category) without Vietnamese text, and text
//     filed under an id that no longer exists,
//   - a phrase code of the core without Vietnamese text (or the other way round), or with other
//     {placeholders} than the English text,
//   - an empty text.
//
// Adding a string therefore means adding its key to every locale file, and nothing else.

import { readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const LOCALES_DIR = 'apps/desktop/src/shared/i18n/locales';
const PLURAL = /\.(zero|one|two|few|many|other)$/;

/** `files.one` and `files.other` are one message with two forms; everything else stands alone. */
function baseOf(key) {
  return key.replace(PLURAL, '');
}

function placeholders(text) {
  return [...String(text).matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort();
}

/** Compares two locale files. `source` is English; `other` is a translation of it. */
export function checkLocales(name, source, other) {
  const problems = [];
  const sourceBases = new Set(Object.keys(source).map(baseOf));
  const otherBases = new Set(Object.keys(other).map(baseOf));
  for (const base of sourceBases) {
    if (!otherBases.has(base)) problems.push(`${name}: missing key "${base}" (it is in en.json)`);
  }
  for (const base of otherBases) {
    if (!sourceBases.has(base)) problems.push(`${name}: key "${base}" is not in en.json`);
  }
  for (const key of Object.keys(source)) {
    if (PLURAL.test(key) && key.endsWith('.other')) continue;
    if (PLURAL.test(key)) {
      // A plural message needs its `other` form in every language.
      if (!(`${baseOf(key)}.other` in source)) {
        problems.push(`en.json: "${key}" has no "${baseOf(key)}.other" form`);
      }
    }
  }
  for (const [key, text] of Object.entries(other)) {
    if (String(text).trim() === '') problems.push(`${name}: "${key}" is empty`);
    if (PLURAL.test(key) && !key.endsWith('.other')) continue;
    const twin = source[key];
    if (twin === undefined) continue;
    const want = placeholders(twin).join(',');
    const have = placeholders(text).join(',');
    if (want !== have) {
      problems.push(`${name}: "${key}" has placeholders {${have}} but en.json has {${want}}`);
    }
  }
  for (const [key, text] of Object.entries(source)) {
    if (String(text).trim() === '') problems.push(`en.json: "${key}" is empty`);
  }
  return problems;
}

/** Ids and categories of the catalogue files, read without a TOML parser. */
export function catalogueIds(toml, tag) {
  const blocks = toml.split(new RegExp(`^\\[\\[${tag}\\]\\]`, 'm')).slice(1);
  return blocks.map((block) => {
    const head = block.split(/^\[/m)[0];
    return {
      id: /^id = "([^"]+)"/m.exec(head)?.[1],
      category: /^category = "([^"]+)"/m.exec(head)?.[1],
    };
  });
}

const camel = (snake) => snake.replace(/_([a-z])/g, (_, c) => c.toUpperCase());

/** Every tweak and rule of the catalogue needs Vietnamese text, and nothing else may be filed. */
export function checkCatalogue(catalogue, tweakToml, ruleToml) {
  const problems = [];
  const expected = new Set();
  for (const { id } of catalogueIds(tweakToml, 'tweak')) {
    if (!id) continue;
    for (const part of ['title', 'description', 'evidence']) expected.add(`tweak.${id}.${part}`);
  }
  for (const { id, category } of catalogueIds(ruleToml, 'rule')) {
    if (id) for (const part of ['title', 'description']) expected.add(`rule.${id}.${part}`);
    if (category) expected.add(`clean.category.${camel(category)}`);
  }
  for (const key of expected) {
    const text = catalogue[key];
    if (text === undefined) problems.push(`vi.catalogue.json: no Vietnamese text for "${key}"`);
    else if (String(text).trim() === '') problems.push(`vi.catalogue.json: "${key}" is empty`);
  }
  for (const key of Object.keys(catalogue)) {
    if (!expected.has(key)) {
      problems.push(`vi.catalogue.json: "${key}" belongs to no tweak, rule or category`);
    }
  }
  return problems;
}

/**
 * The English phrase table of the core: the `("code", "text")` lines of `ENGLISH` in
 * crates/volt-core/src/phrase.rs, as an object. Read without a Rust parser, so keep one entry per
 * pair and no `{` inside a text other than placeholders.
 */
export function englishPhrases(rust) {
  const start = rust.indexOf('pub const ENGLISH');
  if (start < 0) return {};
  const end = rust.indexOf('\n];', start);
  const body = rust.slice(start, end < 0 ? rust.length : end);
  const table = {};
  for (const match of body.matchAll(/\(\s*"([^"]+)",\s*"((?:[^"\\]|\\.)*)"\s*,?\s*\)/g)) {
    table[match[1]] = JSON.parse(`"${match[2]}"`);
  }
  return table;
}

function readJson(path) {
  return JSON.parse(readFileSync(path, 'utf8'));
}

function main() {
  const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
  const dir = join(root, LOCALES_DIR);
  const en = readJson(join(dir, 'en.json'));
  const vi = readJson(join(dir, 'vi.json'));
  const catalogue = readJson(join(dir, 'vi.catalogue.json'));
  const phrases = englishPhrases(
    readFileSync(join(root, 'crates/volt-core/src/phrase.rs'), 'utf8'),
  );
  const viPhrases = readJson(join(dir, 'vi.phrases.json'));
  const problems = [
    ...checkLocales('vi.json', en, vi),
    ...(Object.keys(phrases).length === 0
      ? ['phrase.rs: no English phrase table was found']
      : checkLocales('vi.phrases.json', phrases, viPhrases)),
    ...checkCatalogue(
      catalogue,
      readFileSync(join(root, 'catalogue/tweaks.toml'), 'utf8'),
      readFileSync(join(root, 'catalogue/cleaning.toml'), 'utf8'),
    ),
  ];
  if (problems.length > 0) {
    console.error(`i18n check failed (${problems.length}):`);
    for (const problem of problems) console.error(`  - ${problem}`);
    process.exit(1);
  }
  console.log(
    `i18n check passed: ${Object.keys(en).length} English and ${Object.keys(vi).length} Vietnamese texts agree, ` +
      `${Object.keys(catalogue).length} catalogue texts cover every tweak, rule and category, ` +
      `${Object.keys(phrases).length} core phrases have Vietnamese text.`,
  );
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main();
