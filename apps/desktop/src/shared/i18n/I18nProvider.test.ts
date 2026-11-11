import { describe, expect, it } from 'vitest';
import { guessSystemLocale } from './I18nProvider';

describe('guessSystemLocale', () => {
  it('paints a fresh install in Vietnamese when the window language is Vietnamese', () => {
    expect(guessSystemLocale(['vi-VN', 'en-US'])).toBe('vi');
    expect(guessSystemLocale(['vi'])).toBe('vi');
  });

  it('falls back to English for anything else', () => {
    expect(guessSystemLocale(['en-US'])).toBe('en');
    expect(guessSystemLocale(['fr-FR', 'vi'])).toBe('en');
    expect(guessSystemLocale([])).toBe('en');
    expect(guessSystemLocale(undefined)).toBe('en');
  });
});
