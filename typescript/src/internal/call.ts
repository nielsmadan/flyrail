import {
  AuthorityError,
  type AuthorityErrorKind,
  BundleError,
  type BundleErrorKind,
  FlyrailError,
  HookError,
  NativeError,
  NativePanicError,
  StoreError,
  type StoreErrorKind,
  UnsupportedTranslationError,
  ValueError,
} from '../errors.js';

export type NativeErrorPayload =
  | { readonly type: 'value'; readonly message: string }
  | {
      readonly type: 'bundle';
      readonly kind: 'invalid' | 'io';
      readonly message: string;
      readonly errno: number | null;
    }
  | {
      readonly type: 'authority';
      readonly kind: 'invalid' | 'unsafe' | 'io';
      readonly message: string;
      readonly path: string | null;
      readonly errno: number | null;
    }
  | {
      readonly type: 'store';
      readonly kind: 'busy' | 'invalid-key' | 'io';
      readonly message: string;
      readonly lockName: string | null;
      readonly path: string | null;
      readonly errno: number | null;
    }
  | { readonly type: 'hook'; readonly code: string }
  | { readonly type: 'unsupported-translation'; readonly code: string; readonly message: string }
  | { readonly type: 'argument'; readonly message: string }
  | { readonly type: 'aborted' }
  | { readonly type: 'panic'; readonly message: string }
  | { readonly type: 'native'; readonly message: string };

export type NativeResult<T> =
  | { readonly ok: true; readonly value: T }
  | { readonly ok: false; readonly error: NativeErrorPayload };

export class NativeAborted extends FlyrailError {
  override name = 'NativeAborted';

  constructor() {
    super('native call aborted');
  }
}

const BUNDLE_KINDS: readonly BundleErrorKind[] = ['invalid', 'io'];
const AUTHORITY_KINDS: readonly AuthorityErrorKind[] = ['invalid', 'unsafe', 'io'];
const STORE_KINDS: readonly StoreErrorKind[] = ['busy', 'invalid-key', 'io'];

const knownKind = (kinds: readonly string[], kind: string): boolean => kinds.includes(kind);

const unknownPayload = (payload: NativeErrorPayload): NativeError =>
  new NativeError(`unknown native error payload: ${JSON.stringify(payload)}`);

export function translateNativeError(payload: NativeErrorPayload): FlyrailError {
  switch (payload.type) {
    case 'value':
    case 'argument':
      return new ValueError(payload.message);
    case 'bundle':
      if (!knownKind(BUNDLE_KINDS, payload.kind)) {
        return unknownPayload(payload);
      }
      return new BundleError(payload.kind, payload.message, payload.errno ?? null);
    case 'authority':
      if (!knownKind(AUTHORITY_KINDS, payload.kind)) {
        return unknownPayload(payload);
      }
      return new AuthorityError(
        payload.kind,
        payload.message,
        payload.path ?? null,
        payload.errno ?? null,
      );
    case 'store':
      if (!knownKind(STORE_KINDS, payload.kind)) {
        return unknownPayload(payload);
      }
      return new StoreError(
        payload.kind,
        payload.message,
        payload.lockName ?? null,
        payload.path ?? null,
        payload.errno ?? null,
      );
    case 'hook':
      return new HookError(payload.code);
    case 'unsupported-translation':
      return new UnsupportedTranslationError(payload.code, payload.message);
    case 'aborted':
      return new NativeAborted();
    case 'panic':
      return new NativePanicError(payload.message);
    case 'native':
      return new NativeError(payload.message);
    default:
      return unknownPayload(payload);
  }
}

function foreignNativeError(error: unknown): NativeError {
  const message = error instanceof Error ? error.message : String(error);
  return new NativeError(message, { cause: error });
}

export const asFlyrailError = (error: unknown): FlyrailError =>
  error instanceof FlyrailError ? error : foreignNativeError(error);

const isTagged = (result: unknown): result is NativeResult<unknown> =>
  typeof result === 'object' &&
  result !== null &&
  'ok' in result &&
  (result.ok === true ||
    (result.ok === false &&
      'error' in result &&
      typeof result.error === 'object' &&
      result.error !== null));

const unwrap = <T>(result: NativeResult<T>): T => {
  if (!isTagged(result)) {
    throw new NativeError('native call returned an untagged result');
  }
  if (result.ok) {
    return result.value;
  }
  throw translateNativeError(result.error);
};

export function callNative<T>(operation: () => NativeResult<T>): T {
  let result: NativeResult<T>;
  try {
    result = operation();
  } catch (error) {
    throw asFlyrailError(error);
  }
  return unwrap(result);
}

export function awaitNative<T>(operation: Promise<NativeResult<T>>): Promise<T> {
  return operation.then(unwrap, (error: unknown) => {
    throw asFlyrailError(error);
  });
}
