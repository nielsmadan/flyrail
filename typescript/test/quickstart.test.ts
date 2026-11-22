import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { afterEach, describe, expect, test, vi } from 'vitest';
import { run } from '../examples/quickstart.js';
import { tempRoot } from './helpers/temp.js';
import { exists } from './helpers/files.js';

const firstTypeScriptBlock = (markdown: string): string | undefined =>
  /^```ts\n([\s\S]*?)^```$/mu.exec(markdown)?.[1];

afterEach(() => {
  vi.restoreAllMocks();
});

describe('quickstart', () => {
  test('the README shows examples/quickstart.ts verbatim', async () => {
    const readme = await readFile(new URL('../README.md', import.meta.url), 'utf8');
    const example = await readFile(new URL('../examples/quickstart.ts', import.meta.url), 'utf8');
    expect(firstTypeScriptBlock(readme)?.replace(/\n$/u, '')).toBe(example.replace(/\n$/u, ''));
  });

  test('installs and removes a skill, then applies and removes an AGENTS.md section', async () => {
    const root = await tempRoot();
    const log = vi.spyOn(console, 'log').mockImplementation(() => undefined);
    await run(root);
    expect(log.mock.calls.slice(0, 4)).toEqual([
      ['install:', 'applied'],
      ['current:', true],
      ['uninstall:', 'applied'],
      ['apply:', 'applied'],
    ]);
    expect(log.mock.calls[4]?.[0]).toContain('Run the project checks before committing.');
    expect(log.mock.calls[5]).toEqual(['remove:', 'applied']);
    expect(await exists(join(root, 'AGENTS.md'))).toBe(false);
    expect(await exists(join(root, 'skills', 'review'))).toBe(false);
  });
});
