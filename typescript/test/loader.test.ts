import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, test } from 'vitest';
import { NativeLoadError } from '../src/errors.js';
import { loadNative, loadedFrom, native, nativeTriple } from '../src/native/loader.js';

const cargoVersion = /^version = "([^"]+)"$/m.exec(
  readFileSync(new URL('../native/Cargo.toml', import.meta.url), 'utf8'),
)?.[1];

const fakeLoader = (
  load: (id: string) => unknown,
  resolve: (id: string) => string,
): Parameters<typeof loadNative>[0]['load'] => Object.assign(load, { resolve });

const captureLoadError = (run: () => unknown): NativeLoadError => {
  try {
    run();
  } catch (error) {
    if (error instanceof NativeLoadError) {
      return error;
    }
    throw error;
  }
  throw new Error('expected a NativeLoadError');
};

describe('native loader', () => {
  test('loads the checkout dev build', () => {
    const triple = nativeTriple(process.platform, process.arch);
    expect(loadedFrom).toBe(
      fileURLToPath(new URL(`../build/native/flyrail.${triple}.node`, import.meta.url)),
    );
  });

  test('the loaded binary reports the native crate version', () => {
    expect(native.nativeVersion()).toBe(cargoVersion);
  });

  test.each([
    ['darwin', 'arm64', 'darwin-arm64'],
    ['darwin', 'x64', 'darwin-x64'],
    ['linux', 'arm64', 'linux-arm64-gnu'],
    ['linux', 'x64', 'linux-x64-gnu'],
  ])('maps %s-%s to the %s binary', (platform, arch, triple) => {
    expect(nativeTriple(platform, arch)).toBe(triple);
  });

  test.each([
    ['win32', 'x64'],
    ['linux', 'ia32'],
    ['freebsd', 'arm64'],
  ])('refuses %s-%s before any load attempt', (platform, arch) => {
    const loads: string[] = [];
    const load = fakeLoader(
      (id) => loads.push(id),
      (id) => id,
    );
    const error = captureLoadError(() =>
      loadNative({
        platform,
        arch,
        load,
        devBuild: new URL('file:///dev/build/'),
        version: '1.2.3',
      }),
    );
    expect(error.message).toBe(`flyrail does not support ${platform}-${arch}`);
    expect(loads).toEqual([]);
  });

  test('falls back to the platform package when the dev build is missing', () => {
    const loads: string[] = [];
    const binding = { marker: 'platform package', nativeVersion: () => '1.2.3' };
    const load = fakeLoader(
      (id) => {
        loads.push(id);
        if (id.startsWith('/dev/')) {
          throw new Error('not built');
        }
        return binding;
      },
      (id) => `/app/node_modules/${id}`,
    );
    const loaded = loadNative({
      platform: 'linux',
      arch: 'x64',
      load,
      devBuild: new URL('file:///dev/build/'),
      version: '1.2.3',
    });
    expect(loaded.path).toBe('/app/node_modules/flyrail-linux-x64/flyrail.node');
    expect(loaded.binding).toBe(binding);
    expect(loads).toEqual([
      '/dev/build/flyrail.linux-x64-gnu.node',
      '/app/node_modules/flyrail-linux-x64/flyrail.node',
    ]);
  });

  test('lists every failed attempt', () => {
    const load = fakeLoader(
      () => {
        throw new Error('not built');
      },
      () => {
        throw new Error('not installed');
      },
    );
    const error = captureLoadError(() =>
      loadNative({
        platform: 'darwin',
        arch: 'arm64',
        load,
        devBuild: new URL('file:///dev/build/'),
        version: '1.2.3',
      }),
    );
    expect(error.attempts).toEqual([
      { path: '/dev/build/flyrail.darwin-arm64.node', message: 'not built' },
      { path: 'flyrail-darwin-arm64/flyrail.node', message: 'not installed' },
    ]);
  });

  test('skips a stale binary and falls through to the matching one', () => {
    const current = { nativeVersion: () => '1.2.3' };
    const load = fakeLoader(
      (id) => (id.startsWith('/dev/') ? { nativeVersion: () => '1.0.0' } : current),
      (id) => `/app/node_modules/${id}`,
    );
    const loaded = loadNative({
      platform: 'linux',
      arch: 'x64',
      load,
      devBuild: new URL('file:///dev/build/'),
      version: '1.2.3',
    });
    expect(loaded.binding).toBe(current);
  });

  test('reports a version mismatch when no candidate matches', () => {
    const load = fakeLoader(
      () => ({ nativeVersion: () => '1.0.0' }),
      (id) => id,
    );
    const error = captureLoadError(() =>
      loadNative({
        platform: 'linux',
        arch: 'x64',
        load,
        devBuild: new URL('file:///dev/build/'),
        version: '1.2.3',
      }),
    );
    expect(error.attempts.map((a) => a.message)).toEqual([
      'native addon version 1.0.0 does not match flyrail 1.2.3',
      'native addon version 1.0.0 does not match flyrail 1.2.3',
    ]);
  });
});
