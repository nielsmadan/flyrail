import js from '@eslint/js';
import prettier from 'eslint-config-prettier';
import { defineConfig } from 'eslint/config';
import tseslint from 'typescript-eslint';

const restrictedModules = [
  'child_process',
  'os',
  'net',
  'http',
  'https',
  'readline',
  'worker_threads',
].flatMap((name) => [name, `node:${name}`]);

export default defineConfig(
  {
    ignores: [
      'dist/**',
      'build/**',
      'native/**',
      'node_modules/**',
      'src/native/binding.d.ts',
      'scripts/consumer/**',
    ],
  },
  js.configs.recommended,
  tseslint.configs.strictTypeChecked,
  {
    languageOptions: {
      parserOptions: { projectService: true, tsconfigRootDir: import.meta.dirname },
    },
    rules: {
      '@typescript-eslint/prefer-promise-reject-errors': ['error', { allowThrowingUnknown: true }],
    },
  },
  {
    files: ['**/*.js', '**/*.mjs'],
    extends: [tseslint.configs.disableTypeChecked],
  },
  {
    files: ['src/**/*.ts'],
    rules: {
      'no-console': 'error',
      'no-restricted-imports': [
        'error',
        {
          paths: [
            ...restrictedModules.map((name) => ({
              name,
              message: 'The library never spawns, prompts, reads the host or touches the network.',
            })),
            ...['process', 'node:process'].map((name) => ({
              name,
              importNames: ['env', 'cwd', 'chdir', 'stdout', 'stderr'],
              message: 'The library never reads the host or prints.',
            })),
          ],
        },
      ],
      'no-restricted-syntax': [
        'error',
        {
          selector: "MemberExpression[object.name='globalThis'][property.name='process']",
          message: 'The library never reaches the host through globalThis.process.',
        },
      ],
      'no-restricted-properties': [
        'error',
        { object: 'process', property: 'env', message: 'The library never reads the environment.' },
        {
          object: 'process',
          property: 'cwd',
          message: 'The library never reads the working directory.',
        },
        {
          object: 'process',
          property: 'chdir',
          message: 'The library never changes the working directory.',
        },
        { object: 'process', property: 'stdout', message: 'The library never prints.' },
        { object: 'process', property: 'stderr', message: 'The library never prints.' },
      ],
    },
  },
  prettier,
);
