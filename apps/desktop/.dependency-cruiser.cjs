// Frontend boundary rules, mirroring scripts/check-arch.mjs for crates.
// See the Architecture document: features sit on top of shared and never import each other.
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
      comment: 'shared is the foundation layer: it cannot depend on features or the app root.',
      severity: 'error',
      from: { path: '^src/shared/' },
      to: { path: '^src/(features/|App\\.tsx$|main\\.tsx$)' },
    },
    {
      name: 'features-must-not-import-app-root',
      severity: 'error',
      from: { path: '^src/features/' },
      to: { path: '^src/(App|main)\\.tsx$' },
    },
    { name: 'no-circular', severity: 'error', from: {}, to: { circular: true } },
  ],
  options: {
    tsConfig: { fileName: 'tsconfig.json' },
    doNotFollow: { path: 'node_modules' },
  },
};
