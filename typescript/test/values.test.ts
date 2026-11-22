import { describe, expect, test } from 'vitest';
import {
  DateTimeValue,
  DateValue,
  ObjectValue,
  TimeValue,
  ValueError,
  decodeValue,
  encodeValue,
  valuesEqual,
  type SemanticValue,
} from 'flyrail';
import { callNative } from '../src/internal/call.js';
import { fromNative, toNative, type NativeValue } from '../src/internal/convert.js';
import { native } from '../src/native/loader.js';

const nest = (depth: number): SemanticValue =>
  Array.from({ length: depth }).reduce<SemanticValue>((inner) => [inner], null);

const wireNest = (depth: number): NativeValue =>
  Array.from({ length: depth }).reduce<NativeValue>((inner) => ({ t: 'array', items: [inner] }), {
    t: 'null',
  });

const depthOf = (value: SemanticValue): number => {
  let depth = 0;
  let current: SemanticValue | undefined = value;
  while (Array.isArray(current)) {
    depth += 1;
    current = (current as readonly SemanticValue[])[0];
  }
  return depth;
};

const roundTrip = (value: SemanticValue): SemanticValue => decodeValue(encodeValue(value));

describe('value encoding', () => {
  const date = new DateValue(2026, 1, 2);
  const time = new TimeValue(1, 2, 3, 4);
  const samples: readonly (readonly [string, SemanticValue])[] = [
    ['null', null],
    ['true', true],
    ['false', false],
    ['integer', 42n],
    ['negative integer', -7n],
    ['float', 1.5],
    ['text', 'héllo 𝄞'],
    ['date', date],
    ['time', time],
    ['local datetime', DateTimeValue.local(date, time)],
    ['utc datetime', DateTimeValue.utc(date, time)],
    ['array', [1n, 'two', [3.5, null]]],
    [
      'object',
      new ObjectValue([
        ['b', [true]],
        ['a', new ObjectValue([['c', 1n]])],
      ]),
    ],
  ];

  test.each(samples)('round trips %s', (_, value) => {
    expect(valuesEqual(roundTrip(value), value)).toBe(true);
  });

  test('keeps the sign of negative zero', () => {
    expect(Object.is(roundTrip(-0), -0)).toBe(true);
  });

  test('keeps the smallest subnormal', () => {
    expect(roundTrip(Number.MIN_VALUE)).toBe(Number.MIN_VALUE);
  });

  test('keeps the signed 64 bit limits', () => {
    expect(roundTrip(2n ** 63n - 1n)).toBe(2n ** 63n - 1n);
    expect(roundTrip(-(2n ** 63n))).toBe(-(2n ** 63n));
  });

  test('encodes the documented tagged text', () => {
    expect(new TextDecoder().decode(encodeValue(['x', 1n]))).toBe(
      '["array",[["string","x"],["integer","1"]]]',
    );
  });

  test('decodes object members in sorted order', () => {
    const decoded = decodeValue(
      new TextEncoder().encode('["object",[["a",["integer","1"]],["b",["null"]]]]'),
    );
    expect(decoded).toBeInstanceOf(ObjectValue);
    expect((decoded as ObjectValue).entries().map(([key]) => key)).toEqual(['a', 'b']);
  });

  test('refuses bytes that are not a canonical record', () => {
    expect(() => decodeValue(new TextEncoder().encode('["integer","01"]'))).toThrow(ValueError);
  });
});

describe('value validation', () => {
  test('refuses an integer past 64 bits', () => {
    expect(() => toNative(2n ** 63n)).toThrow(new ValueError('integers must fit signed 64 bits'));
    expect(() => toNative(-(2n ** 63n) - 1n)).toThrow(ValueError);
  });

  test.each([NaN, Infinity, -Infinity])('refuses the float %s', (value) => {
    expect(() => encodeValue(value)).toThrow(new ValueError('floats must be finite'));
  });

  test('refuses text with a lone surrogate', () => {
    expect(() => encodeValue('a\ud800')).toThrow(ValueError);
  });

  test('refuses values that are not semantic values', () => {
    expect(() => encodeValue({} as unknown as SemanticValue)).toThrow(ValueError);
    expect(() => encodeValue(undefined as unknown as SemanticValue)).toThrow(ValueError);
  });

  test('distinguishes zero from negative zero and integers from floats', () => {
    expect(valuesEqual(0, -0)).toBe(false);
    expect(valuesEqual(1.5, 1.5)).toBe(true);
    expect(valuesEqual(1n, 1)).toBe(false);
  });
});

describe('ObjectValue', () => {
  test('refuses a duplicate key with the Rust message', () => {
    expect(
      () =>
        new ObjectValue([
          ['a', 1n],
          ['a', 2n],
        ]),
    ).toThrow(new ValueError('duplicate object key'));
  });

  test('orders keys by UTF-8 bytes, not UTF-16 units', () => {
    const forward = new ObjectValue([
      ['\u{1F600}', 1n],
      ['￿', 2n],
    ]);
    const reverse = new ObjectValue([
      ['￿', 2n],
      ['\u{1F600}', 1n],
    ]);
    expect(forward.entries().map(([key]) => key)).toEqual(['￿', '\u{1F600}']);
    expect(encodeValue(forward)).toEqual(encodeValue(reverse));
  });

  test('looks members up by key', () => {
    const object = new ObjectValue([['k', 7n]]);
    expect(object.size).toBe(1);
    expect(object.has('k')).toBe(true);
    expect(object.get('k')).toBe(7n);
    expect(object.get('missing')).toBeUndefined();
    expect([...object]).toEqual([['k', 7n]]);
  });

  test('refuses a key with a lone surrogate', () => {
    expect(() => new ObjectValue([['\udc00', 1n]])).toThrow(ValueError);
  });
});

describe('deep values', () => {
  test('a 512-deep value survives a native round trip', () => {
    const deep = nest(512);
    const echoed = callNative(() => native.internalEchoValue(toNative(deep)));
    const restored = fromNative(echoed);
    expect(depthOf(restored)).toBe(512);
    expect(valuesEqual(restored, deep)).toBe(true);
  });

  test('a 512-deep value encodes', () => {
    const expected = '["array",['.repeat(512) + '["null"]' + ']]'.repeat(512);
    expect(new TextDecoder().decode(encodeValue(nest(512)))).toBe(expected);
  });

  test('a 200-deep value survives encode and decode', () => {
    expect(depthOf(roundTrip(nest(200)))).toBe(200);
  });

  test('toNative refuses depth 513 before any native call', () => {
    expect(() => toNative(nest(513))).toThrow(new ValueError('value nesting is too deep'));
  });

  test('decodeValue refuses input nested past about 256 levels', () => {
    expect(() => decodeValue(encodeValue(nest(257)))).toThrow(
      new ValueError('JSON nesting is too deep'),
    );
  });

  test('the native reader refuses a hand-built integer beyond signed 64 bits', () => {
    const result = native.valuesEncode({ t: 'int', v: 2n ** 63n });
    expect(result).toMatchObject({
      ok: false,
      error: { message: 'integers must fit signed 64 bits' },
    });
  });

  test('the native encoder refuses a hand-built 513-deep tree', () => {
    const result = native.valuesEncode(wireNest(513));
    expect(result).toMatchObject({
      ok: false,
      error: { type: 'argument', message: 'value nesting is too deep' },
    });
    expect(() => callNative(() => native.valuesEncode(wireNest(513)))).toThrow(ValueError);
  });
});
