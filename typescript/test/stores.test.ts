import { chmod, mkdir } from 'node:fs/promises';
import { join } from 'node:path';
import { describe, expect, test } from 'vitest';
import {
  DirectoryStore,
  InstallationTarget,
  MemoryStore,
  RenderContext,
  StoreError,
  Target,
  ValueError,
  validateKey,
  validateLockName,
} from 'flyrail';
import type { StateStore } from 'flyrail';
import { tempRoot } from './helpers/temp.js';

const bytes = (text: string): Uint8Array => new TextEncoder().encode(text);

describe('MemoryStore', () => {
  test('write and delete are compare-and-set', async () => {
    const store = new MemoryStore();
    expect(await store.write('records/a', bytes('one'), null)).toBe(true);
    expect(await store.write('records/a', bytes('two'), bytes('wrong'))).toBe(false);
    expect(await store.read('records/a')).toEqual(bytes('one'));
    expect(await store.write('records/a', bytes('two'), bytes('one'))).toBe(true);
    expect(await store.delete('records/a', bytes('one'))).toBe(false);
    expect(await store.delete('records/a', bytes('two'))).toBe(true);
    expect(await store.read('records/a')).toBeNull();
  });

  test('lists keys by prefix', async () => {
    const store = new MemoryStore();
    await store.write('index/a', bytes('1'), null);
    await store.write('index/b', bytes('2'), null);
    await store.write('other/c', bytes('3'), null);
    expect(await store.keys('index/')).toEqual(['index/a', 'index/b']);
  });

  test('has no filesystem footprint', () => {
    const store = new MemoryStore();
    expect([store.location, store.stagingDirectory]).toEqual([null, null]);
  });

  test('an invalid key rejects with StoreError', async () => {
    await expect(new MemoryStore().read('Bad')).rejects.toMatchObject({
      name: 'StoreError',
      kind: 'invalid-key',
      message: 'state key must be slash-separated lowercase segments',
    });
  });

  test('write checks an aborted signal before it starts', async () => {
    const reason = new Error('stop');
    const store = new MemoryStore();
    await expect(
      store.write('records/a', bytes('one'), null, { signal: AbortSignal.abort(reason) }),
    ).rejects.toBe(reason);
    expect(await store.read('records/a')).toBeNull();
  });
});

describe('DirectoryStore', () => {
  test('a relative path rejects with ValueError', async () => {
    await expect(DirectoryStore.open('rel')).rejects.toThrow(
      new ValueError('store path must be an absolute path'),
    );
  });

  test('stores records under its location', async () => {
    const root = await tempRoot();
    const store = await DirectoryStore.open(join(root, 'state'));
    expect(store.location).toBe(join(root, 'state'));
    expect(store.stagingDirectory).toBe(join(root, 'state', 'staging'));
    expect(await store.write('records/a', bytes('one'), null)).toBe(true);
    expect(await store.keys('')).toEqual(['records/a']);
    const reopened = await DirectoryStore.open(join(root, 'state'));
    expect(await reopened.read('records/a')).toEqual(bytes('one'));
  });

  test('open does no I/O, so a public location is refused on first use', async () => {
    const root = await tempRoot();
    const parent = join(root, 'parent');
    const location = join(parent, 'state');
    await mkdir(location, { recursive: true });
    await chmod(parent, 0o755);
    await chmod(location, 0o777);
    const store = await DirectoryStore.open(location);
    const failure = store.keys('');
    await expect(failure).rejects.toBeInstanceOf(StoreError);
    await expect(failure).rejects.toMatchObject({
      kind: 'io',
      message: expect.stringContaining('must be private to its owner') as unknown,
    });
  });
});

describe('key validation', () => {
  test('validateKey throws StoreError for an invalid key', () => {
    expect(() => {
      validateKey('Bad');
    }).toThrow(
      expect.objectContaining({
        name: 'StoreError',
        kind: 'invalid-key',
        message: 'state key must be slash-separated lowercase segments',
      }),
    );
  });

  test('validateLockName names the lock in its message', () => {
    expect(() => {
      validateLockName('a//b');
    }).toThrow('lock name must be slash-separated lowercase segments');
  });

  test('well-formed keys and lock names are accepted', () => {
    expect(() => {
      validateKey('index/team/abc');
      validateLockName('index/team/abc');
    }).not.toThrow();
  });
});

describe('InstallationTarget', () => {
  test('equality needs the same store object and equal contexts', () => {
    const store = new MemoryStore();
    const context = { agent: 'codex' };
    expect(
      new InstallationTarget(store, context).equals(new InstallationTarget(store, context)),
    ).toBe(true);
    expect(
      new InstallationTarget(store, context).equals(
        new InstallationTarget(new MemoryStore(), context),
      ),
    ).toBe(false);
    expect(new InstallationTarget(store).equals(new InstallationTarget(store, context))).toBe(
      false,
    );
  });

  test('exposes the store, sorted contexts and digest', () => {
    const store = new MemoryStore();
    const target = new InstallationTarget(store, { b: '2', a: '1' }, { route: 'x' });
    expect(target.store).toBe(store);
    expect(Object.keys(target.context)).toEqual(['a', 'b']);
    expect(target.routingContext).toEqual({ route: 'x' });
    expect(target.contextDigest).toMatch(/^[0-9a-f]{64}$/);
  });

  test('a store that is not a built-in store throws ValueError', () => {
    expect(() => new InstallationTarget({} as StateStore)).toThrow(
      new ValueError('store must be a DirectoryStore or MemoryStore'),
    );
  });

  test('a render context derives its stable identity', () => {
    const store = new MemoryStore();
    const context = new RenderContext(Target.project('codex', '/work/app'), 'linux');
    const target = context.installation(store);
    expect(target.store).toBe(store);
    expect(target.context).toEqual({
      agent: 'codex',
      root: '/work/app',
      scope: 'project',
      surface: 'cli',
    });
    expect(context.installation(store, { id: 'fixed' }).context).toEqual({ id: 'fixed' });
  });
});
