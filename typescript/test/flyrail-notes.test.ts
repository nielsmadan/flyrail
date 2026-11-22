import { cp } from 'node:fs/promises';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, test } from 'vitest';
import { run } from '../examples/flyrail-notes/main.js';
import { tempRoot } from './helpers/temp.js';

interface Captured {
  readonly code: number;
  readonly stdout: string;
  readonly stderr: string;
}

const bundle = fileURLToPath(new URL('../examples/flyrail-notes/bundle', import.meta.url));

const cli = async (argv: readonly string[], signal?: AbortSignal): Promise<Captured> => {
  const stdout: string[] = [];
  const stderr: string[] = [];
  const code = await run(argv, {
    stdout: { write: (text: string) => stdout.push(text) },
    stderr: { write: (text: string) => stderr.push(text) },
    ...(signal === undefined ? {} : { signal }),
  });
  return { code, stdout: stdout.join(''), stderr: stderr.join('') };
};

const report = (captured: Captured): { results: Record<string, unknown>[] } =>
  JSON.parse(captured.stdout) as { results: Record<string, unknown>[] };

describe('flyrail-notes', () => {
  test('installs, reports current status and uninstalls a skill', async () => {
    const root = await tempRoot();
    const source = join(root, 'source');
    await cp(bundle, source, { recursive: true });
    const targets = ['--target', join(root, 'skills'), '--state', join(root, 'state')];

    const installed = await cli(['ai', 'install', '--source', source, ...targets]);
    expect(installed.code).toBe(0);
    expect(report(installed).results[0]?.status).toBe('applied');

    const status = await cli(['ai', 'status', '--source', source, ...targets]);
    expect(status.code).toBe(0);
    expect(report(status).results[0]?.is_current).toBe(true);

    const removed = await cli(['ai', 'uninstall', '--bundle-id', 'example-notes', ...targets]);
    expect(removed.code).toBe(0);
    expect(report(removed).results[0]?.status).toBe('applied');
  });

  test('a missing --state is a usage error', async () => {
    const root = await tempRoot();
    const result = await cli([
      'ai',
      'install',
      '--source',
      bundle,
      '--target',
      join(root, 'skills'),
    ]);
    expect(result.code).toBe(2);
    expect(result.stderr).toMatch(/^flyrail-notes: --state is required\nUsage:\n/u);
  });

  test('an aborted install exits 1 with the interruption message', async () => {
    const root = await tempRoot();
    const source = join(root, 'source');
    await cp(bundle, source, { recursive: true });
    const result = await cli(
      [
        'ai',
        'install',
        '--source',
        source,
        '--target',
        join(root, 'skills'),
        '--state',
        join(root, 'state'),
      ],
      AbortSignal.abort(),
    );
    expect(result.code).toBe(1);
    expect(result.stderr).toBe('flyrail-notes: interrupted before any change was committed\n');
  });
});
