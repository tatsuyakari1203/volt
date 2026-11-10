import { afterEach, describe, expect, it } from 'vitest';
import { formatSize } from '../text/size';
import {
  formatNumber,
  parseChoice,
  readLanguageOverride,
  resolveLocale,
  setLocale,
  t,
  tn,
  tx,
} from './translate';

afterEach(() => setLocale('en'));

describe('t', () => {
  it('writes English by default and Vietnamese after the language changes', () => {
    expect(t('home.boost')).toBe('Boost');
    expect(t('nav.home')).toBe('Home');
    setLocale('vi');
    expect(t('nav.home')).toBe('Trang chủ');
  });

  it('fills the placeholders and formats numbers for the language', () => {
    expect(t('home.deductions.points', { points: 12 })).toBe('−12 points');
    expect(t('count.file.other', { count: 12345 })).toBe('12,345 files');
    setLocale('vi');
    expect(t('count.file.other', { count: 12345 })).toBe('12.345 tệp');
    expect(t('about.version', { version: '0.1.0' })).toBe('Phiên bản 0.1.0');
  });

  it('leaves a placeholder alone when no value is given for it', () => {
    expect(t('about.version')).toBe('Version {version}');
  });
});

describe('tn', () => {
  it('chooses the singular and the plural form in English', () => {
    expect(tn('count.file', 1)).toBe('1 file');
    expect(tn('count.file', 0)).toBe('0 files');
    expect(tn('count.file', 2)).toBe('2 files');
  });

  it('uses the one form that Vietnamese has for every count', () => {
    setLocale('vi');
    expect(tn('count.file', 1)).toBe('1 tệp');
    expect(tn('count.file', 2)).toBe('2 tệp');
  });
});

describe('tx', () => {
  it('shows the English of the core in English, even when a translation exists', () => {
    expect(tx('tweak.show-file-extensions.title', 'The core says this')).toBe('The core says this');
  });

  it('looks the text up by its stable id in Vietnamese and falls back to the core', () => {
    setLocale('vi');
    expect(tx('tweak.show-file-extensions.title', 'Show file extensions')).toBe('Hiện đuôi tệp');
    expect(tx('tweak.unknown.title', 'A tweak added later')).toBe('A tweak added later');
  });
});

describe('numbers and sizes', () => {
  it('follow the decimal mark of the language', () => {
    expect(formatSize(3.4 * 1024 ** 2)).toBe('3.4 MB');
    expect(formatNumber(1234.5)).toBe('1,234.5');
    setLocale('vi');
    expect(formatSize(3.4 * 1024 ** 2)).toBe('3,4 MB');
    expect(formatNumber(1234.5)).toBe('1.234,5');
  });
});

describe('the choice', () => {
  it('follows the first language of the system for System and obeys any other choice', () => {
    expect(resolveLocale('system', ['vi-VN', 'en-US'])).toBe('vi');
    expect(resolveLocale('system', ['vi'])).toBe('vi');
    expect(resolveLocale('system', ['en-US', 'vi'])).toBe('en');
    expect(resolveLocale('system', ['fr-FR'])).toBe('en');
    expect(resolveLocale('system', [])).toBe('en');
    expect(resolveLocale('en', ['vi-VN'])).toBe('en');
    expect(resolveLocale('vi', ['en-US'])).toBe('vi');
  });

  it('accepts only the three known values', () => {
    expect(parseChoice('system')).toBe('system');
    expect(parseChoice('vi')).toBe('vi');
    expect(parseChoice('fr')).toBeNull();
    expect(parseChoice(null)).toBeNull();
  });

  it('reads a ?lang= override that never means System', () => {
    expect(readLanguageOverride('?lang=vi')).toBe('vi');
    expect(readLanguageOverride('?lang=system')).toBeNull();
    expect(readLanguageOverride('')).toBeNull();
  });
});
