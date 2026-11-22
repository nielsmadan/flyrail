import { join } from 'node:path';
import { describe, expect, test } from 'vitest';
import { InstallationTarget, Target, canonicalDestination, install } from 'flyrail';
import { holdLock, storeUnder } from './helpers/lifecycle.js';
import { skillBundle } from './helpers/skills.js';
import { tempRoot } from './helpers/temp.js';
import { exists } from './helpers/files.js';

describe('skill cancellation', () => {
  test('an abort during a blocked index lock wait rejects with the reason and installs nothing', async () => {
    const root = await tempRoot();
    const store = await storeUnder(root);
    const bundle = skillBundle();
    const targets = [Target.directory(join(root, 'skills'))];
    const destination = await canonicalDestination(join(root, 'skills'));
    const { contextDigest } = new InstallationTarget(store, { destination });
    const hold = await holdLock(store, `index/${bundle.id}/${contextDigest}`);
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
