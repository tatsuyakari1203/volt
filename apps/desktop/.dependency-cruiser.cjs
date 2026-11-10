// Frontend boundary rules, mirroring scripts/check-arch.mjs for crates.
// See the Architecture document: features sit on top of shared and never import each other; the
// app shell (src/app, src/App.tsx, src/main.tsx) sits on top of both.
/** @type {import('dependency-cruiser').IConfiguration} */
module.exports = {
  forbidden: [
    {
      name: 'no-cross-feature-imports',
      comment: 'A feature may only import from itself and from shared.',
      severity: 'error',
      from: { path: '^src/features/([^/]+)/' },
      to: { path: '^src/features/([^/]+)/', pathNot: '^src/features/$1/' },
    },
    {
      name: 'shared-must-not-import-features',
      comment: 'shared is the foundation layer: it cannot depend on features or the app shell.',
      severity: 'error',
      from: { path: '^src/shared/' },
      to: { path: '^src/(features/|app/|App\\.tsx$|main\\.tsx$)' },
    },
    {
      name: 'features-must-not-import-app-shell',
      comment: 'Features are mounted by the app shell, never the other way round.',
      severity: 'error',
      from: { path: '^src/features/' },
      to: { path: '^src/(app/|App\\.tsx$|main\\.tsx$)' },
    },
    {
      name: 'app-shell-is-imported-from-the-root-only',
      comment: 'src/app is composed by App.tsx; nothing else reaches into it.',
      severity: 'error',
      from: { path: '^src/', pathNot: '^src/(app/|App\\.tsx$|App\\.test\\.tsx$)' },
      to: { path: '^src/app/' },
    },
    { name: 'no-circular', severity: 'error', from: {}, to: { circular: true } },
  ],
  options: {
    tsConfig: { fileName: 'tsconfig.json' },
    doNotFollow: { path: 'node_modules' },
  },
};
