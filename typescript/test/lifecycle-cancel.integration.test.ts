import { join } from 'node:path';
import { describe, expect, test, vi } from 'vitest';
import { inspectInstallation, sync } from 'flyrail';
import { codexProject, holdLock, storeUnder } from './helpers/lifecycle.js';
import { tempRoot } from './helpers/temp.js';
import { exists } from './helpers/files.js';

const timeout = 30_000;

describe('lifecycle cancellation', () => {
  test(
    'an abort during a blocked lock wait rejects with the exact reason and writes nothing',
    async () => {
      const root = await tempRoot();
      const store = await storeUnder(root);
      const { bundle, rendered, target } = await codexProject(join(root, 'app'), store, 'team');
      const hold = await holdLock(store, `index/team/${target.contextDigest}`);
      try {
        const controller = new AbortController();
        const reason = new Error('stop');
        const started = performance.now();
        setTimeout(() => {
          controller.abort(reason);
        }, 100);
        await expect(
          sync(bundle, rendered, target, { lockTimeoutMs: 10_000, signal: controller.signal }),
        ).rejects.toBe(reason);
        expect(performance.now() - started).toBeLessThan(1000);
        expect(await exists(join(root, 'app', 'AGENTS.md'))).toBe(false);
      } finally {
        hold.release();
      }
      const observed = await inspectInstallation('team', target);
      expect(observed.version).toBeNull();
      expect(observed.pending).toBe(false);
      expect(observed.error).toBeNull();
      expect(await store.keys('index/')).toEqual([]);
    },
    timeout,
  );

  test(
    'a blocked lock wait without an abort times out as busy',
    async () => {
      const root = await tempRoot();
      const store = await storeUnder(root);
      const { bundle, rendered, target } = await codexProject(join(root, 'app'), store, 'team');
      const hold = await holdLock(store, `index/team/${target.contextDigest}`);
      try {
        const result = await sync(bundle, rendered, target, { lockTimeoutMs: 100 });
        expect(result.status).toBe('failed');
        expect(result.error?.code).toBe('busy');
      } finally {
        hold.release();
      }
    },
    timeout,
  );

  test(
    'a settled call leaves no abort listener behind',
    async () => {
      const root = await tempRoot();
      const { bundle, rendered, target } = await codexProject(
        join(root, 'app'),
        await storeUnder(root),
      );
      const { signal } = new AbortController();
      const added = vi.spyOn(signal, 'addEventListener');
      const removed = vi.spyOn(signal, 'removeEventListener');
      const result = await sync(bundle, rendered, target, { signal, lockTimeoutMs: 10_000 });
      expect(result.status).toBe('applied');
      expect(added).toHaveBeenCalledTimes(1);
      expect(removed.mock.calls.map((call) => call[1])).toEqual(
        added.mock.calls.map((call) => call[1]),
      );
    },
    timeout,
  );

  test(
    'eight concurrent syncs sharing one store all apply',
    async () => {
      const root = await tempRoot();
      const store = await storeUnder(root);
      const projects = await Promise.all(
        Array.from({ length: 8 }, (_, index) =>
          codexProject(join(root, `app-${String(index)}`), store),
        ),
      );
      const results = await Promise.all(
        projects.map(({ bundle, rendered, target }) =>
          sync(bundle, rendered, target, { lockTimeoutMs: 10_000 }),
        ),
      );
      const outcomes = results.map(
        (result) => `${result.status}:${result.error?.code ?? ''}:${result.error?.message ?? ''}`,
      );
      expect(outcomes).toEqual(Array(8).fill('applied::'));
    },
    timeout,
  );
});
