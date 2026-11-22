import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { describe, expect, test } from 'vitest';
import {
  Bundle,
  BundleIdentity,
  InstructionArtifact,
  applyLoadout,
  discoverLoadout,
  previewLoadout,
  recoverLoadout,
  type LoadoutLocation,
  type LoadoutPreview,
  type StateStore,
} from 'flyrail';
import type { NativeLockHold } from '../src/native/binding.js';
import { holdLock, storeUnder } from './helpers/lifecycle.js';
import { fixtureRoot, writeFiles } from './helpers/loadout.js';

const timeout = 30_000;
const original = "# keep me\n[[source]]\nname='local'\npath='.'\n[claude]\ninstructions=[]\n";

const prepare = async (
  locked: boolean,
): Promise<{
  root: string;
  store: StateStore;
  location: LoadoutLocation;
  proposal: LoadoutPreview;
  lock: () => Promise<NativeLockHold>;
  hold: NativeLockHold | null;
}> => {
  const root = await fixtureRoot();
  await writeFiles(root, {
    'home/.config/loadout/config.toml': "source='{root}/source'\n",
    'source/loadout.toml': original,
  });
  const found = await discoverLoadout({ home: join(root, 'home'), environment: {} });
  if (found.status !== 'available') {
    throw new Error(`discovery was ${found.status}`);
  }
  const store = await storeUnder(root);
  const bundle = Bundle.fromArtifacts(new BundleIdentity('loadout-probe', 'autumn'), [
    new InstructionArtifact('guide', 'Read the diff.\n'),
  ]);
  const destination = found.setup.select('local', ['claude']);
  const { location } = destination;
  const { contextDigest } = location.installation(store);
  const lock = (): Promise<NativeLockHold> =>
    holdLock(store, `index/loadout-probe/${contextDigest}`);
  const hold = locked ? await lock() : null;
  const proposal = await previewLoadout(bundle, destination, store, { platform: 'macos' });
  return { root, store, location, proposal, lock, hold };
};

const held = async (): Promise<{
  root: string;
  proposal: LoadoutPreview;
  hold: NativeLockHold;
}> => {
  const { root, proposal, hold } = await prepare(true);
  if (hold === null) {
    throw new Error('lock was not held');
  }
  return { root, proposal, hold };
};

const sourceText = (root: string): Promise<string> =>
  readFile(join(root, 'source/loadout.toml'), 'utf8');

const abortAfter = (controller: AbortController, reason: Error): void => {
  setTimeout(() => {
    controller.abort(reason);
  }, 100);
};

const releaseAfter = (hold: NativeLockHold): void => {
  setTimeout(() => {
    hold.release();
  }, 150);
};

describe('loadout apply cancellation', () => {
  test(
    'an abort during a blocked applyLoadout rejects with the reason and writes nothing',
    async () => {
      const { root, proposal, hold } = await held();
      try {
        const controller = new AbortController();
        const reason = new Error('stop');
        abortAfter(controller, reason);
        await expect(
          applyLoadout(proposal, { lockTimeoutMs: 10_000, signal: controller.signal }),
        ).rejects.toBe(reason);
      } finally {
        hold.release();
      }
      expect(await sourceText(root)).toBe(original);
    },
    timeout,
  );

  test(
    'a short lock timeout fails applyLoadout as busy and writes nothing',
    async () => {
      const { root, proposal, hold } = await held();
      try {
        const result = await applyLoadout(proposal, { lockTimeoutMs: 100 });
        expect(result.lifecycle.status).toBe('failed');
        expect(result.lifecycle.error?.code).toBe('busy');
      } finally {
        hold.release();
      }
      expect(await sourceText(root)).toBe(original);
    },
    timeout,
  );

  test(
    'applyLoadout waits within its lock timeout for a lock released meanwhile',
    async () => {
      const { proposal, hold } = await held();
      try {
        releaseAfter(hold);
        const result = await applyLoadout(proposal, { lockTimeoutMs: 10_000 });
        expect(result.lifecycle.status).toBe('applied');
      } finally {
        hold.release();
      }
    },
    timeout,
  );
});

describe('loadout recover cancellation', () => {
  const installed = async (): Promise<{
    root: string;
    store: StateStore;
    location: LoadoutLocation;
    lock: () => Promise<NativeLockHold>;
    applied: string;
  }> => {
    const { root, store, location, proposal, lock } = await prepare(false);
    const result = await applyLoadout(proposal);
    if (result.lifecycle.status !== 'applied') {
      throw new Error(`setup apply was ${result.lifecycle.status}`);
    }
    return { root, store, location, lock, applied: await sourceText(root) };
  };

  test(
    'an abort during a blocked recoverLoadout rejects with the reason and writes nothing',
    async () => {
      const { root, store, location, lock, applied } = await installed();
      const hold = await lock();
      try {
        const controller = new AbortController();
        const reason = new Error('stop');
        abortAfter(controller, reason);
        await expect(
          recoverLoadout('loadout-probe', location, store, {
            lockTimeoutMs: 10_000,
            signal: controller.signal,
          }),
        ).rejects.toBe(reason);
      } finally {
        hold.release();
      }
      expect(await sourceText(root)).toBe(applied);
    },
    timeout,
  );

  test(
    'a short lock timeout fails recoverLoadout as busy',
    async () => {
      const { store, location, lock } = await installed();
      const hold = await lock();
      try {
        const result = await recoverLoadout('loadout-probe', location, store, {
          lockTimeoutMs: 100,
        });
        expect(result.lifecycle.status).toBe('failed');
        expect(result.lifecycle.error?.code).toBe('busy');
      } finally {
        hold.release();
      }
    },
    timeout,
  );

  test(
    'recoverLoadout waits within its lock timeout for a lock released meanwhile',
    async () => {
      const { store, location, lock } = await installed();
      const hold = await lock();
      try {
        releaseAfter(hold);
        const result = await recoverLoadout('loadout-probe', location, store, {
          lockTimeoutMs: 10_000,
        });
        expect(result.lifecycle.status).toBe('unchanged');
      } finally {
        hold.release();
      }
    },
    timeout,
  );
});
