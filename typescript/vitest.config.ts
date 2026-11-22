import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vitest/config';

const fastTimeoutMs = 10_000;

export default defineConfig({
  resolve: {
    alias: { flyrail: fileURLToPath(new URL('./src/index.ts', import.meta.url)) },
  },
  test: {
    projects: [
      {
        extends: true,
        test: {
          name: 'unit',
          include: ['test/**/*.test.ts'],
          exclude: ['test/**/*.integration.test.ts', 'test/conformance/**', 'test/smoke.test.ts'],
          pool: 'forks',
          testTimeout: fastTimeoutMs,
        },
      },
      {
        extends: true,
        test: {
          name: 'smoke',
          include: ['test/smoke.test.ts'],
          pool: 'forks',
          testTimeout: fastTimeoutMs,
        },
      },
      {
        extends: true,
        test: {
          name: 'conformance',
          include: ['test/conformance/**/*.test.ts'],
          pool: 'forks',
          testTimeout: fastTimeoutMs,
        },
      },
      {
        extends: true,
        test: {
          name: 'integration',
          include: ['test/**/*.integration.test.ts'],
          pool: 'forks',
          testTimeout: 60_000,
        },
      },
    ],
  },
});
