import { spawnSync } from 'node:child_process';
import { describe, expect, test } from 'vitest';
import { NativePanicError } from '../src/errors.js';
import { awaitNative, callNative } from '../src/internal/call.js';
import { loadedFrom, native } from '../src/native/loader.js';
import { requireTool } from './helpers/tools.js';

const childScript = `
import { createRequire } from 'node:module';
const binding = createRequire(import.meta.url)(process.argv[1]);
const result = await binding.internalPanicAsync();
process.stdout.write(result.error.message);
`;

describe('worker pool', () => {
  test('runs more jobs at once than the libuv pool has threads', async () => {
    const joined = await Promise.all(
      Array.from({ length: 8 }, () => awaitNative(native.internalRendezvous(8, 5000))),
    );
    expect(joined).toEqual(Array.from({ length: 8 }, () => true));
  });

  test('a rendezvous that never fills times out', async () => {
    await expect(awaitNative(native.internalRendezvous(2, 50))).resolves.toBe(false);
  });

  test('a worker panic rejects with NativePanicError and the pool keeps working', async () => {
    await expect(awaitNative(native.internalPanicAsync())).rejects.toThrow(
      new NativePanicError('flyrail internal panic'),
    );
    await expect(awaitNative(native.internalBlock(10))).resolves.toBeUndefined();
  });

  test('a synchronous panic throws NativePanicError', () => {
    expect(() => callNative(() => native.internalPanic())).toThrow(
      new NativePanicError('flyrail internal panic'),
    );
  });

  test('panics reach JS without printing to stderr', () => {
    const child = spawnSync(
      requireTool('node'),
      ['--input-type=module', '--eval', childScript, loadedFrom],
      { encoding: 'utf8', timeout: 10_000 },
    );
    expect(child.stdout).toBe('flyrail internal panic');
    expect(child.status).toBe(0);
    expect(child.stderr).toBe('');
  });
});
