import { describe, expect, test } from 'vitest';
import { fromPairs, toPairs } from '../src/internal/maps.js';

describe('maps', () => {
  test('toPairs lists record entries in insertion order', () => {
    expect(toPairs({ PATH: '/bin', HOME: '/home/a' })).toEqual([
      ['PATH', '/bin'],
      ['HOME', '/home/a'],
    ]);
  });

  test('toPairs maps an absent record to no pairs', () => {
    expect(toPairs(undefined)).toEqual([]);
  });

  test('fromPairs builds a frozen record', () => {
    const record = fromPairs([
      ['a', 1],
      ['b', 2],
    ]);
    expect(record).toEqual({ a: 1, b: 2 });
    expect(Object.isFrozen(record)).toBe(true);
  });
});
