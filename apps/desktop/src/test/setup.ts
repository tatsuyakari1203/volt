import { cleanup, configure } from '@testing-library/react';
import { afterEach } from 'vitest';
import { setLocale } from '../shared/i18n/translate';

// findBy* and waitFor give up after one second by default. On a machine that is building, a
// screen can take longer than that to settle, so this one limit covers every test; a test that is
// really waiting for something that never happens still fails, only later.
configure({ asyncUtilTimeout: 10_000 });

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
