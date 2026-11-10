import { expect, it } from 'vitest';
import {
  applyTheme,
  parsePreference,
  readQueryOverride,
  readStoredPreference,
  resolveTheme,
  THEME_STORAGE_KEY,
} from './theme';

it('follows the system when the preference is system', () => {
  expect(resolveTheme('system', true)).toBe('dark');
  expect(resolveTheme('system', false)).toBe('light');
});

it('a manual preference beats the system', () => {
  expect(resolveTheme('light', true)).toBe('light');
  expect(resolveTheme('dark', false)).toBe('dark');
});

it('accepts only light or dark in the ?theme= override', () => {
  expect(readQueryOverride('?theme=dark')).toBe('dark');
  expect(readQueryOverride('?a=1&theme=light')).toBe('light');
  expect(readQueryOverride('?theme=system')).toBeNull();
  expect(readQueryOverride('?theme=purple')).toBeNull();
  expect(readQueryOverride('')).toBeNull();
});

it('reads a stored preference and falls back to system on junk or errors', () => {
  expect(readStoredPreference({ getItem: () => 'dark' })).toBe('dark');
  expect(readStoredPreference({ getItem: () => 'neon' })).toBe('system');
  expect(readStoredPreference({ getItem: () => null })).toBe('system');
  expect(readStoredPreference(null)).toBe('system');
  expect(
    readStoredPreference({
      getItem: () => {
        throw new Error('blocked');
      },
    }),
  ).toBe('system');
  expect(THEME_STORAGE_KEY).toBe('volt.theme');
  expect(parsePreference(42)).toBeNull();
});

it('applies the resolved theme as a data attribute', () => {
  applyTheme('dark');
  expect(document.documentElement.dataset.theme).toBe('dark');
});
