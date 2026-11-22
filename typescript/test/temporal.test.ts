import { describe, expect, test } from 'vitest';
import { DateTimeValue, DateValue, TimeValue, ValueError } from 'flyrail';

describe('DateValue', () => {
  test('parses and prints a leap day', () => {
    const date = DateValue.parse('2024-02-29');
    expect([date.year, date.month, date.day]).toEqual([2024, 2, 29]);
    expect(date.toString()).toBe('2024-02-29');
  });

  test('refuses a day outside its month with the Rust message', () => {
    expect(() => new DateValue(2023, 2, 29)).toThrow(
      new ValueError('date day is out of range for its month'),
    );
  });

  test('refuses a fractional or oversized field', () => {
    expect(() => new DateValue(2024, 1.5, 1)).toThrow(ValueError);
    expect(() => new DateValue(70_000, 1, 1)).toThrow(new ValueError('date year must be 1-9999'));
  });

  test('compares by field', () => {
    expect(new DateValue(2024, 1, 2).equals(DateValue.parse('2024-01-02'))).toBe(true);
    expect(new DateValue(2024, 1, 2).equals(new DateValue(2024, 1, 3))).toBe(false);
  });
});

describe('TimeValue', () => {
  test('defaults microseconds to zero and prints six digits', () => {
    const time = new TimeValue(1, 2, 3);
    expect(time.microsecond).toBe(0);
    expect(time.toString()).toBe('01:02:03.000000');
  });

  test('parses fractional seconds', () => {
    const time = TimeValue.parse('23:59:59.999999');
    expect([time.hour, time.minute, time.second, time.microsecond]).toEqual([23, 59, 59, 999_999]);
  });

  test('refuses an hour of 24', () => {
    expect(() => new TimeValue(24, 0, 0)).toThrow(ValueError);
  });
});

describe('DateTimeValue', () => {
  const date = new DateValue(2024, 2, 29);
  const time = new TimeValue(10, 0, 0);

  test('local values carry no offset', () => {
    const local = DateTimeValue.local(date, time);
    expect(local.isUtc).toBe(false);
    expect(local.toString()).toBe('2024-02-29T10:00:00.000000');
  });

  test('utc values print the zero offset', () => {
    const utc = DateTimeValue.utc(date, time);
    expect(utc.isUtc).toBe(true);
    expect(utc.toString()).toBe('2024-02-29T10:00:00.000000+00:00');
  });

  test('an offset normalizes to utc', () => {
    const shifted = DateTimeValue.withOffset(date, time, 3600);
    expect(shifted.isUtc).toBe(true);
    expect(shifted.toString()).toBe('2024-02-29T09:00:00.000000+00:00');
  });

  test('parse applies the offset and equals compares the result', () => {
    const parsed = DateTimeValue.parse('2024-02-29T11:00:00+02:00');
    expect(parsed.equals(DateTimeValue.withOffset(date, new TimeValue(11, 0, 0), 7200))).toBe(true);
    expect(parsed.equals(DateTimeValue.local(date, time))).toBe(false);
  });

  test('an offset of a day or more is refused', () => {
    expect(() => DateTimeValue.withOffset(date, time, 86_400)).toThrow(ValueError);
  });
});
