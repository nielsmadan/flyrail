import type { ErrorCode } from './enums.js';

export class FlyrailError extends Error {
  override name = 'FlyrailError';
}

export class ValueError extends FlyrailError {
  override name = 'ValueError';
}

export type BundleErrorKind = 'invalid' | 'io';

export class BundleError extends FlyrailError {
  override name = 'BundleError';
  readonly kind: BundleErrorKind;
  readonly errno: number | null;

  constructor(kind: BundleErrorKind, message: string, errno: number | null = null) {
    super(message);
    this.kind = kind;
    this.errno = errno;
  }
}

export type AuthorityErrorKind = 'invalid' | 'unsafe' | 'io';

export class AuthorityError extends FlyrailError {
  override name = 'AuthorityError';
  readonly kind: AuthorityErrorKind;
  readonly path: string | null;
  readonly errno: number | null;

  constructor(
    kind: AuthorityErrorKind,
    message: string,
    path: string | null = null,
    errno: number | null = null,
  ) {
    super(message);
    this.kind = kind;
    this.path = path;
    this.errno = errno;
  }
}

export type StoreErrorKind = 'busy' | 'invalid-key' | 'io';

export class StoreError extends FlyrailError {
  override name = 'StoreError';
  readonly kind: StoreErrorKind;
  readonly lockName: string | null;
  readonly path: string | null;
  readonly errno: number | null;

  constructor(
    kind: StoreErrorKind,
    message: string,
    lockName: string | null = null,
    path: string | null = null,
    errno: number | null = null,
  ) {
    super(message);
    this.kind = kind;
    this.lockName = lockName;
    this.path = path;
    this.errno = errno;
  }
}

export class HookError extends FlyrailError {
  override name = 'HookError';
  readonly code: string;

  constructor(code: string) {
    super(`flyrail-hook:${code}`);
    this.code = code;
  }
}

export class UnsupportedTranslationError extends FlyrailError {
  override name = 'UnsupportedTranslationError';
  readonly code: string;

  constructor(code: string, message: string) {
    super(message);
    this.code = code;
  }
}

export class TargetError extends FlyrailError {
  override name = 'TargetError';
  readonly code: ErrorCode;
  readonly path: string | null;
  readonly errno: number | null;

  constructor(
    code: ErrorCode,
    message: string,
    path: string | null = null,
    errno: number | null = null,
  ) {
    super(message);
    this.code = code;
    this.path = path;
    this.errno = errno;
  }
}

export interface NativeLoadAttempt {
  readonly path: string;
  readonly message: string;
}

export class NativeLoadError extends FlyrailError {
  override name = 'NativeLoadError';
  readonly attempts: readonly NativeLoadAttempt[];

  constructor(message: string, attempts: readonly NativeLoadAttempt[] = []) {
    super(message);
    this.attempts = Object.freeze(attempts.map((attempt) => Object.freeze({ ...attempt })));
  }
}

export class NativePanicError extends FlyrailError {
  override name = 'NativePanicError';
}

export class NativeError extends FlyrailError {
  override name = 'NativeError';
}
