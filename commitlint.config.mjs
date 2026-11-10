// Conventional Commits. Scopes map to crates and repo areas; see the Devflow doc.
export default {
  extends: ['@commitlint/config-conventional'],
  rules: {
    'scope-enum': [
      2,
      'always',
      [
        'core',
        'win',
        'store',
        'desktop',
        'ui',
        'cli',
        'backlog',
        'devflow',
        'ci',
        'deps',
        'docs',
        'release',
      ],
    ],
    'body-max-line-length': [1, 'always', 120],
  },
};
