// Tests for scripts/check-i18n.mjs. Run with `node --test scripts/`.
// Each kind of gap gets a case that must fail, plus cases that must stay quiet.

import assert from 'node:assert/strict';
import { describe, it } from 'node:test';

import { catalogueIds, checkCatalogue, checkLocales, englishPhrases } from './check-i18n.mjs';

const en = {
  'home.title': 'Home',
  'files.one': '{count} file',
  'files.other': '{count} files',
  'size.line': '{used} of {total}',
};
const vi = {
  'home.title': 'Trang chủ',
  'files.other': '{count} tệp',
  'size.line': '{used} trên {total}',
};

describe('locale files', () => {
  it('accept a language that has the same keys, with one plural form fewer', () => {
    assert.deepEqual(checkLocales('vi.json', en, vi), []);
  });

  it('fail when a key is missing from the translation', () => {
    const { 'home.title': _gone, ...rest } = vi;
    const problems = checkLocales('vi.json', en, rest);
    assert.equal(problems.length, 1);
    assert.match(problems[0], /missing key "home\.title"/);
  });

  it('fail when the translation has a key English lacks', () => {
    const problems = checkLocales('vi.json', en, { ...vi, 'home.extra': 'Thêm' });
    assert.match(problems.join('\n'), /"home\.extra" is not in en\.json/);
  });

  it('fail when a plural message has no other form in the translation', () => {
    const { 'files.other': _gone, ...rest } = vi;
    assert.match(checkLocales('vi.json', en, rest).join('\n'), /missing key "files"/);
  });

  it('fail when a placeholder is dropped or misspelled', () => {
    const problems = checkLocales('vi.json', en, { ...vi, 'size.line': '{used} trên {totl}' });
    assert.match(problems.join('\n'), /"size\.line" has placeholders/);
  });

  it('fail on an empty text', () => {
    assert.match(checkLocales('vi.json', en, { ...vi, 'home.title': ' ' }).join('\n'), /empty/);
  });
});

const TWEAKS = `
[[tweak]]
id = "dark-mode"
category = "appearance"

[[tweak.steps]]
kind = "registry_set"
`;
const RULES = `
[[rule]]
id = "user-temp"
category = "temporary_files"

[rule.root]
location = "user_temp"
`;
const FULL = {
  'tweak.dark-mode.title': 'a',
  'tweak.dark-mode.description': 'b',
  'tweak.dark-mode.evidence': 'c',
  'rule.user-temp.title': 'd',
  'rule.user-temp.description': 'e',
  'clean.category.temporaryFiles': 'f',
};

describe('catalogue texts', () => {
  it('read the ids and categories of the catalogue files', () => {
    assert.deepEqual(catalogueIds(TWEAKS, 'tweak'), [{ id: 'dark-mode', category: 'appearance' }]);
    assert.deepEqual(catalogueIds(RULES, 'rule'), [
      { id: 'user-temp', category: 'temporary_files' },
    ]);
  });

  it('accept a catalogue with Vietnamese text for every tweak, rule and category', () => {
    assert.deepEqual(checkCatalogue(FULL, TWEAKS, RULES), []);
  });

  it('fail when a tweak id has no Vietnamese text', () => {
    const { 'tweak.dark-mode.evidence': _gone, ...rest } = FULL;
    assert.match(checkCatalogue(rest, TWEAKS, RULES).join('\n'), /tweak\.dark-mode\.evidence/);
  });

  it('fail when a cleaning rule id or its category has no Vietnamese text', () => {
    const { 'rule.user-temp.title': _a, 'clean.category.temporaryFiles': _b, ...rest } = FULL;
    const problems = checkCatalogue(rest, TWEAKS, RULES).join('\n');
    assert.match(problems, /rule\.user-temp\.title/);
    assert.match(problems, /clean\.category\.temporaryFiles/);
  });

  it('fail when text is filed under an id that no longer exists', () => {
    assert.match(
      checkCatalogue({ ...FULL, 'tweak.gone.title': 'x' }, TWEAKS, RULES).join('\n'),
      /belongs to no tweak/,
    );
  });
});

describe('phrases of the core', () => {
  const rust = `
pub const ENGLISH: &[(&str, &str)] = &[
    // A comment.
    ("class.error", "error"),
    ("count.file.one", "{count} file"),
    ("count.file.other", "{count} files"),
    ("summary.note", " (it said \\"no\\")"),
    (
        "split.over.lines",
        "{a} and {b}",
    ),
];
`;

  it('are read from the Rust table, with escapes and entries over several lines', () => {
    assert.deepEqual(englishPhrases(rust), {
      'class.error': 'error',
      'count.file.one': '{count} file',
      'count.file.other': '{count} files',
      'summary.note': ' (it said "no")',
      'split.over.lines': '{a} and {b}',
    });
    assert.deepEqual(englishPhrases('fn main() {}'), {});
  });

  it('must have Vietnamese text with the same placeholders', () => {
    const english = englishPhrases(rust);
    const ok = {
      'class.error': 'lỗi',
      'count.file.other': '{count} tệp',
      'summary.note': ' (nói "không")',
      'split.over.lines': '{a} và {b}',
    };
    assert.deepEqual(checkLocales('vi.phrases.json', english, ok), []);
    const { 'class.error': _gone, ...rest } = ok;
    assert.match(
      checkLocales('vi.phrases.json', english, rest).join('\n'),
      /missing key "class\.error"/,
    );
    assert.match(
      checkLocales('vi.phrases.json', english, { ...ok, 'split.over.lines': '{a} và {c}' }).join(
        '\n',
      ),
      /placeholders \{a,c\}/,
    );
  });
});
