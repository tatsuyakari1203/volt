import { cleanup } from '@testing-library/react';
import { afterEach } from 'vitest';
import { setLocale } from '../shared/i18n/translate';

// Vitest does not expose globals, so Testing Library cannot register its own cleanup.
afterEach(() => {
  cleanup();
  // The translator holds the language in the module: every test starts in English.
  setLocale('en');
  document.documentElement.removeAttribute('lang');
  document.documentElement.removeAttribute('data-theme');
  try {
    window.localStorage.clear();
  } catch {
    // Storage may be unavailable; tests do not depend on it.
  }
});
