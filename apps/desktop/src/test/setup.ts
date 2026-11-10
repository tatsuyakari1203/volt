import { cleanup } from '@testing-library/react';
import { afterEach } from 'vitest';

// Vitest does not expose globals, so Testing Library cannot register its own cleanup.
afterEach(() => {
  cleanup();
  document.documentElement.removeAttribute('data-theme');
  try {
    window.localStorage.clear();
  } catch {
    // Storage may be unavailable; tests do not depend on it.
  }
});
