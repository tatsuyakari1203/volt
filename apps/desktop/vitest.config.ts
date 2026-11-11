import react from '@vitejs/plugin-react';
import { defineConfig } from 'vitest/config';

export default defineConfig({
  plugins: [react()],
  test: {
    environment: 'jsdom',
    include: ['src/**/*.test.{ts,tsx}'],
    setupFiles: ['src/test/setup.ts'],
    // Screen tests render whole pages; on a loaded CI runner or a machine that is building they
    // can take several times longer than when idle. Slow tests are fixed first; this one limit
    // covers the rest, so no test sets its own.
    testTimeout: 30_000,
    // Let tokens.test.ts read index.css as text (`?raw`); other CSS stays stubbed out.
    css: { include: [/\.css\?raw$/] },
  },
});
