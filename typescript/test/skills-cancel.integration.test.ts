import { access } from 'node:fs/promises';
import { join } from 'node:path';
import { describe, expect, test } from 'vitest';
import { Target, install, uninstall } from 'flyrail';
import { holdLock, storeUnder } from './helpers/lifecycle.js';
import { skillBundle } from './helpers/skills.js';
import { tempRoot } from './helpers/temp.js';

const exists = (path: string): Promise<boolean> =>
  access(path).then(
    () => true,
    () => false,
  );

describe('skill cancellation', () => {
  test('an abort during a blocked index lock wait rejects with the reason and installs nothing', async () => {
    const root = await tempRoot();
    const store = await storeUnder(root);
    const bundle = skillBundle();
    const targets = [Target.directory(join(root, 'skills'))];
    await install(bundle, targets, store);
    await uninstall(bundle.id, targets, store);
    const indexKeys = await store.keys(`index/${bundle.id}/`);
    expect(indexKeys).toHaveLength(1);
    const [lockName] = indexKeys;
    const hold = await holdLock(store, lockName ?? '');
    try {
      const controller = new AbortController();
      const reason = new Error('stop');
      const started = performance.now();
      setTimeout(() => {
        controller.abort(reason);
      }, 100);
      await expect(
        install(bundle, targets, store, { lockTimeoutMs: 10_000, signal: controller.signal }),
      ).rejects.toBe(reason);
      expect(performance.now() - started).toBeLessThan(1000);
      expect(await exists(join(root, 'skills', 'review'))).toBe(false);
    } finally {
      hold.release();
    }
  }, 30_000);
});
