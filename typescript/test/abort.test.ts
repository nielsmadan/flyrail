import { describe, expect, test, vi } from 'vitest';
import { ValueError } from '../src/errors.js';
import { lockTimeout, runMutation, runReadOnly } from '../src/internal/abort.js';
import { awaitNative } from '../src/internal/call.js';
import { native } from '../src/native/loader.js';

const abortAfter = (controller: AbortController, ms: number, reason: unknown): void => {
  setTimeout(() => {
    controller.abort(reason);
  }, ms);
};

describe('runMutation', () => {
  test('rejects with the exact signal reason when a blocked call is aborted', async () => {
    const controller = new AbortController();
    const reason = new Error('stop');
    const started = performance.now();
    abortAfter(controller, 50, reason);
    const outcome = runMutation(controller.signal, (token) =>
      awaitNative(native.internalBlock(5000, token)),
    );
    await expect(outcome).rejects.toBe(reason);
    expect(performance.now() - started).toBeLessThan(500);
  });

  test('resolves with the real result when the call finishes after the signal fired', async () => {
    const controller = new AbortController();
    const outcome = runMutation(controller.signal, () => {
      controller.abort(new Error('late'));
      return Promise.resolve('applied');
    });
    await expect(outcome).resolves.toBe('applied');
  });

  test('resolves an unaborted call', async () => {
    await expect(
      runMutation(undefined, (token) => awaitNative(native.internalBlock(30, token))),
    ).resolves.toBeUndefined();
  });
});

describe('runReadOnly', () => {
  test('rejects as soon as the signal aborts while native work continues', async () => {
    const controller = new AbortController();
    const reason = new Error('stop reading');
    abortAfter(controller, 20, reason);
    let finished = false;
    const work = awaitNative(native.internalBlock(100)).then((value) => {
      finished = true;
      return value;
    });
    const outcome = runReadOnly(controller.signal, () => work);
    await expect(outcome).rejects.toBe(reason);
    expect(finished).toBe(false);
    await expect(work).resolves.toBeUndefined();
  });
});

describe('queued jobs', () => {
  test('a job cancelled while queued rejects with the signal reason once a worker reaches it', async () => {
    const blockers = Array.from({ length: 64 }, () => awaitNative(native.internalBlock(300)));
    const controller = new AbortController();
    const reason = new Error('queued stop');
    const outcome = runMutation(controller.signal, (token) =>
      awaitNative(native.internalCancelProbe(token)),
    );
    controller.abort(reason);
    await expect(outcome).rejects.toBe(reason);
    await Promise.all(blockers);
  });
});

describe('already-aborted signals', () => {
  test('reject without calling native', async () => {
    const block = vi.spyOn(native, 'internalBlock');
    const controller = new AbortController();
    const reason = new Error('early');
    controller.abort(reason);
    await expect(
      runMutation(controller.signal, (token) => awaitNative(native.internalBlock(10, token))),
    ).rejects.toBe(reason);
    await expect(
      runReadOnly(controller.signal, () => awaitNative(native.internalBlock(10))),
    ).rejects.toBe(reason);
    expect(block).not.toHaveBeenCalled();
    block.mockRestore();
  });
});

describe('abort listeners', () => {
  test('every listener added is removed once the calls settle', async () => {
    const controller = new AbortController();
    const added = vi.spyOn(controller.signal, 'addEventListener');
    const removed = vi.spyOn(controller.signal, 'removeEventListener');
    const reason = new Error('stop');
    await expect(
      runMutation(controller.signal, (token) => awaitNative(native.internalBlock(10, token))),
    ).resolves.toBeUndefined();
    const reading = runReadOnly(controller.signal, () => awaitNative(native.internalBlock(200)));
    controller.abort(reason);
    await expect(reading).rejects.toBe(reason);
    const listeners = (calls: readonly (readonly unknown[])[]): unknown[][] =>
      calls.map(([type, listener]) => [type, listener]);
    expect(added).toHaveBeenCalledTimes(2);
    expect(listeners(removed.mock.calls)).toEqual(
      expect.arrayContaining(listeners(added.mock.calls)),
    );
  });
});

describe('lockTimeout', () => {
  test.each([-1, Number.NaN, Number.POSITIVE_INFINITY])('rejects %d', (lockTimeoutMs) => {
    expect(() => lockTimeout({ lockTimeoutMs })).toThrow(
      new ValueError('lock timeout must be finite and nonnegative'),
    );
  });

  test('returns the requested timeout', () => {
    expect(lockTimeout({ lockTimeoutMs: 250 })).toBe(250);
  });

  test('defaults to a single attempt', () => {
    expect(lockTimeout(undefined)).toBe(0);
  });

  test.each([1e300, Number.MAX_VALUE])(
    'accepts the finite timeout %d as a very long wait',
    (lockTimeoutMs) => {
      expect(lockTimeout({ lockTimeoutMs })).toBe(Number.MAX_SAFE_INTEGER);
    },
  );
});
