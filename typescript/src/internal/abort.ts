import { ValueError } from '../errors.js';
import { native } from '../native/loader.js';
import type { LockOptions } from '../options.js';
import type { CancelToken as NativeCancelToken } from '../native/binding.js';
import { NativeAborted, asFlyrailError } from './call.js';

export type { NativeCancelToken };

const begin = <T>(start: () => Promise<T>): Promise<T> => {
  try {
    return start().catch((error: unknown) => {
      throw asFlyrailError(error);
    });
  } catch (error) {
    return Promise.reject(asFlyrailError(error));
  }
};

const abortedReason = (signal: AbortSignal): unknown => signal.reason;

const settleOrAbort = <T>(
  signal: AbortSignal,
  pending: Promise<T>,
  onAbort: (reject: (reason: unknown) => void) => void,
): Promise<T> =>
  new Promise<T>((resolve, reject) => {
    const abort = (): void => {
      signal.removeEventListener('abort', abort);
      onAbort(reject);
    };
    signal.addEventListener('abort', abort, { once: true });
    pending.then(
      (value) => {
        signal.removeEventListener('abort', abort);
        resolve(value);
      },
      (error: unknown) => {
        signal.removeEventListener('abort', abort);
        reject(error instanceof NativeAborted ? abortedReason(signal) : error);
      },
    );
  });

export function runMutation<T>(
  signal: AbortSignal | undefined,
  start: (token: NativeCancelToken | null) => Promise<T>,
): Promise<T> {
  if (signal === undefined) {
    return begin(() => start(null));
  }
  if (signal.aborted) {
    return Promise.reject(abortedReason(signal));
  }
  const token = new native.CancelToken();
  return settleOrAbort(
    signal,
    begin(() => start(token)),
    () => {
      token.cancel();
    },
  );
}

export function runReadOnly<T>(
  signal: AbortSignal | undefined,
  start: () => Promise<T>,
): Promise<T> {
  if (signal === undefined) {
    return begin(start);
  }
  if (signal.aborted) {
    return Promise.reject(abortedReason(signal));
  }
  return settleOrAbort(signal, begin(start), (reject) => {
    reject(abortedReason(signal));
  });
}

const LONGEST_LOCK_TIMEOUT_MS = Number.MAX_SAFE_INTEGER;

export function lockTimeout(options: LockOptions | undefined): number {
  const timeout = options?.lockTimeoutMs ?? 0;
  if (!Number.isFinite(timeout) || timeout < 0) {
    throw new ValueError('lock timeout must be finite and nonnegative');
  }
  return Math.min(timeout, LONGEST_LOCK_TIMEOUT_MS);
}
