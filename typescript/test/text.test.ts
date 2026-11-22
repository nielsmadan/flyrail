import { describe, expect, test } from 'vitest';
import { ValueError } from '../src/errors.js';
import { fromUnits, requireText, toUnits } from '../src/internal/text.js';

describe('requireText', () => {
  test('rejects a lone surrogate', () => {
    expect(() => requireText('\uD800', 'id')).toThrow(
      new ValueError('id must be well-formed Unicode text'),
    );
  });

  test.each<unknown>([123, undefined, null, {}])('rejects the non-string %j', (value) => {
    expect(() => requireText(value as string, 'id')).toThrow(new ValueError('id must be a string'));
  });

  test('returns well-formed text unchanged', () => {
    expect(requireText('café \u{1F600}', 'id')).toBe('café \u{1F600}');
  });
});

describe('UTF-16 units', () => {
  test('round-trip a lone surrogate exactly', () => {
    const text = 'a\uD800b\uDC00';
    expect(Array.from(toUnits(text))).toEqual([0x61, 0xd800, 0x62, 0xdc00]);
    expect(fromUnits(toUnits(text))).toBe(text);
  });

  test('round-trip text longer than one conversion chunk', () => {
    const text = 'x\uDFFF'.repeat(10_000);
    expect(fromUnits(toUnits(text))).toBe(text);
  });
});
