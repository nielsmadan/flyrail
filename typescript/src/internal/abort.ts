import { FlyrailError, ValueError } from '../errors.js';
import { native } from '../native/loader.js';
import type { CancelToken as NativeCancelToken } from '../native/binding.js';
import { NativeAborted, foreignNativeError } from './call.js';

export type { NativeCancelToken };

export interface CallOptions {
  readonly signal?: AbortSignal;
}

export interface LockOptions extends CallOptions {
  readonly lockTimeoutMs?: number;
}

const begin = <T>(start: () => Promise<T>): Promise<T> => {
  const wrap = (error: unknown): FlyrailError =>
    error instanceof FlyrailError ? error : foreignNativeError(error);
  try {
    return start().catch((error: unknown) => {
      throw wrap(error);
    });
  } catch (error) {
    return Promise.reject(wrap(error));
  }
};

const abortedReason = (signal: AbortSignal): unknown => signal.reason;

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
  const onAbort = (): void => {
    token.cancel();
  };
  signal.addEventListener('abort', onAbort, { once: true });
  return begin(() => start(token))
    .catch((error: unknown) => {
      throw error instanceof NativeAborted ? abortedReason(signal) : error;
    })
    .finally(() => {
      signal.removeEventListener('abort', onAbort);
    });
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
  const pending = begin(start);
  return new Promise<T>((resolve, reject) => {
    const settle = (): void => {
      signal.removeEventListener('abort', onAbort);
    };
    const onAbort = (): void => {
      settle();
      pending.catch(() => undefined);
      reject(abortedReason(signal));
    };
    signal.addEventListener('abort', onAbort, { once: true });
    pending.then(
      (value) => {
        settle();
        resolve(value);
      },
      (error: unknown) => {
        settle();
        reject(error instanceof NativeAborted ? abortedReason(signal) : error);
      },
    );
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
