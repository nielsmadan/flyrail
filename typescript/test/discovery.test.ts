import { existsSync } from 'node:fs';
import { chmod, mkdir, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { describe, expect, test } from 'vitest';
import { ValueError, detectAgents } from 'flyrail';
import { tempRoot } from './helpers/temp.js';

describe('detectAgents', () => {
  test('finds an executable on the explicit search path', async () => {
    const root = await tempRoot();
    const bin = join(root, 'bin');
    await mkdir(bin);
    await writeFile(join(bin, 'codex'), '#!/bin/sh\n');
    await chmod(join(bin, 'codex'), 0o700);
    expect(await detectAgents({ searchPath: [bin], noApplications: true })).toEqual([
      { agent: 'codex', executable: join(bin, 'codex'), application: null },
    ]);
  });

  const onMac = process.platform === 'darwin';
  const systemCursor = onMac && existsSync('/Applications/Cursor.app');

  test.runIf(onMac && !systemCursor)(
    'the default applications include <home>/Applications',
    async () => {
      const home = join(await tempRoot(), 'home');
      await mkdir(join(home, 'Applications', 'Cursor.app'), { recursive: true });
      const found = await detectAgents({ searchPath: [], home });
      expect(found.find((presence) => presence.agent === 'cursor')?.application).toBe(
        join(home, 'Applications', 'Cursor.app'),
      );
    },
  );

  test.runIf(onMac)('an explicit applications list replaces the defaults', async () => {
    const custom = join(await tempRoot(), 'custom');
    await mkdir(join(custom, 'Cursor.app'), { recursive: true });
    const found = await detectAgents({ searchPath: [], applications: [custom] });
    expect(found).toEqual([
      { agent: 'cursor', executable: null, application: join(custom, 'Cursor.app') },
    ]);
  });

  test('an empty search path finds nothing', async () => {
    expect(await detectAgents({ searchPath: [], noApplications: true })).toEqual([]);
  });

  test.each([
    ['relative', ['bin']],
    ['empty', ['']],
    ['colon', ['/a:/b']],
  ])('a %s search path entry rejects with ValueError', async (_label, searchPath) => {
    await expect(detectAgents({ searchPath })).rejects.toBeInstanceOf(ValueError);
  });

  test('a relative home rejects with ValueError', async () => {
    await expect(detectAgents({ searchPath: [], home: 'me' })).rejects.toBeInstanceOf(ValueError);
  });

  test('an aborted signal rejects with its reason', async () => {
    const controller = new AbortController();
    const reason = new Error('stop');
    controller.abort(reason);
    await expect(detectAgents({ searchPath: [], signal: controller.signal })).rejects.toBe(reason);
  });
});
