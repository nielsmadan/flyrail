import { describe, expect, test } from 'vitest';
import {
  AuthorityError,
  BundleError,
  FlyrailError,
  HookError,
  NativeError,
  NativePanicError,
  StoreError,
  TargetError,
  UnsupportedTranslationError,
  ValueError,
} from '../src/errors.js';
import {
  NativeAborted,
  awaitNative,
  callNative,
  translateNativeError,
  type NativeErrorPayload,
  type NativeResult,
} from '../src/internal/call.js';

type ErrorClass = abstract new (...args: never[]) => FlyrailError;

const cases: readonly (readonly [
  NativeErrorPayload,
  ErrorClass,
  Readonly<Record<string, unknown>>,
])[] = [
  [{ type: 'value', message: 'bad value' }, ValueError, { message: 'bad value' }],
  [{ type: 'argument', message: 'bad argument' }, ValueError, { message: 'bad argument' }],
  [
    { type: 'bundle', kind: 'io', message: 'no such file', errno: 2 },
    BundleError,
    { kind: 'io', message: 'no such file', errno: 2 },
  ],
  [
    { type: 'authority', kind: 'unsafe', message: 'symlink', path: '/tmp/link', errno: null },
    AuthorityError,
    { kind: 'unsafe', message: 'symlink', path: '/tmp/link', errno: null },
  ],
  [
    {
      type: 'store',
      kind: 'busy',
      message: 'state store lock is busy: index/team',
      lockName: 'index/team',
      path: null,
      errno: null,
    },
    StoreError,
    { kind: 'busy', lockName: 'index/team', path: null, errno: null },
  ],
  [
    { type: 'hook', code: 'invalid-json' },
    HookError,
    { code: 'invalid-json', message: 'flyrail-hook:invalid-json' },
  ],
  [
    { type: 'unsupported-translation', code: 'remote', message: 'remote servers are unsupported' },
    UnsupportedTranslationError,
    { code: 'remote', message: 'remote servers are unsupported' },
  ],
  [{ type: 'panic', message: 'boom' }, NativePanicError, { message: 'boom' }],
  [{ type: 'native', message: 'no worker' }, NativeError, { message: 'no worker' }],
  [{ type: 'aborted' }, NativeAborted, {}],
];

describe('translateNativeError', () => {
  test.each(cases)('maps %o', (payload, expected, fields) => {
    const error = translateNativeError(payload);
    expect(error).toBeInstanceOf(expected);
    expect(error).toBeInstanceOf(FlyrailError);
    expect(error).toMatchObject(fields);
  });

  test('normalizes absent optional fields to null', () => {
    const payload = {
      type: 'store',
      kind: 'io',
      message: 'denied',
    } as unknown as NativeErrorPayload;
    expect(translateNativeError(payload)).toMatchObject({
      kind: 'io',
      lockName: null,
      path: null,
      errno: null,
    });
  });

  test.each(['bundle', 'authority', 'store'])(
    'reports an unknown %s kind as NativeError',
    (type) => {
      const payload = { type, kind: 'renamed', message: 'x' } as unknown as NativeErrorPayload;
      const error = translateNativeError(payload);
      expect(error).toBeInstanceOf(NativeError);
      expect(error.message).toBe(`unknown native error payload: ${JSON.stringify(payload)}`);
    },
  );

  test.each([
    [new ValueError('x'), 'ValueError'],
    [new BundleError('invalid', 'x'), 'BundleError'],
    [new TargetError('busy', 'x', '/tmp', 16), 'TargetError'],
    [new NativeError('x'), 'NativeError'],
  ])('gives %o a stable name', (error, name) => {
    expect(error.name).toBe(name);
  });
});

describe('callNative', () => {
  test('returns the value of an ok result', () => {
    expect(callNative(() => ({ ok: true, value: 42 }))).toBe(42);
  });

  test('throws the mapped class for an error result', () => {
    const failure: NativeResult<number> = {
      ok: false,
      error: { type: 'value', message: 'not a value' },
    };
    expect(() => callNative(() => failure)).toThrow(new ValueError('not a value'));
  });

  test('wraps a thrown foreign error in NativeError', () => {
    expect(() =>
      callNative(() => {
        throw new Error('boom');
      }),
    ).toThrow(new NativeError('boom'));
  });

  test('wraps an untagged result in NativeError', () => {
    expect(() => callNative(() => 'raw' as unknown as NativeResult<string>)).toThrow(NativeError);
  });
});

describe('awaitNative', () => {
  test('resolves with the value of an ok result', async () => {
    await expect(awaitNative(Promise.resolve({ ok: true, value: 'done' }))).resolves.toBe('done');
  });

  test('rejects with the mapped class for an error result', async () => {
    const failure: NativeResult<string> = {
      ok: false,
      error: { type: 'panic', message: 'worker panicked' },
    };
    await expect(awaitNative(Promise.resolve(failure))).rejects.toThrow(
      new NativePanicError('worker panicked'),
    );
  });

  test('wraps a foreign rejection in NativeError', async () => {
    const rejection = awaitNative<string>(Promise.reject(new Error('boom')));
    await expect(rejection).rejects.toBeInstanceOf(NativeError);
    await expect(rejection).rejects.toThrow('boom');
  });
});
