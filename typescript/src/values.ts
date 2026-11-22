import { ValueError } from './errors.js';
import { callNative } from './internal/call.js';
import { fromNative, toNative } from './internal/convert.js';
import { requireText } from './internal/text.js';
import type { JsDateTime } from './native/binding.js';
import { native } from './native/loader.js';

export type Binary64 = number;

export class DateValue {
  readonly year: number;
  readonly month: number;
  readonly day: number;
  readonly #text: string;

  constructor(year: number, month: number, day: number) {
    const snapshot = callNative(() => native.temporalDate(year, month, day));
    this.year = snapshot.year;
    this.month = snapshot.month;
    this.day = snapshot.day;
    this.#text = snapshot.text;
    Object.freeze(this);
  }

  static parse(text: string): DateValue {
    const snapshot = callNative(() => native.temporalDateParse(requireText(text, 'date text')));
    return new DateValue(snapshot.year, snapshot.month, snapshot.day);
  }

  equals(other: DateValue): boolean {
    return this.year === other.year && this.month === other.month && this.day === other.day;
  }

  toString(): string {
    return this.#text;
  }
}

export class TimeValue {
  readonly hour: number;
  readonly minute: number;
  readonly second: number;
  readonly microsecond: number;
  readonly #text: string;

  constructor(hour: number, minute: number, second: number, microsecond = 0) {
    const snapshot = callNative(() => native.temporalTime(hour, minute, second, microsecond));
    this.hour = snapshot.hour;
    this.minute = snapshot.minute;
    this.second = snapshot.second;
    this.microsecond = snapshot.microsecond;
    this.#text = snapshot.text;
    Object.freeze(this);
  }

  static parse(text: string): TimeValue {
    const snapshot = callNative(() => native.temporalTimeParse(requireText(text, 'time text')));
    return new TimeValue(snapshot.hour, snapshot.minute, snapshot.second, snapshot.microsecond);
  }

  equals(other: TimeValue): boolean {
    return (
      this.hour === other.hour &&
      this.minute === other.minute &&
      this.second === other.second &&
      this.microsecond === other.microsecond
    );
  }

  toString(): string {
    return this.#text;
  }
}

export class DateTimeValue {
  readonly date: DateValue;
  readonly time: TimeValue;
  readonly isUtc: boolean;
  readonly #text: string;

  private constructor(snapshot: JsDateTime) {
    const { date, time } = snapshot;
    this.date = new DateValue(date.year, date.month, date.day);
    this.time = new TimeValue(time.hour, time.minute, time.second, time.microsecond);
    this.isUtc = snapshot.utc;
    this.#text = snapshot.text;
    Object.freeze(this);
  }

  static local(date: DateValue, time: TimeValue): DateTimeValue {
    return new DateTimeValue(callNative(() => native.temporalDateTimeLocal(date, time)));
  }

  static utc(date: DateValue, time: TimeValue): DateTimeValue {
    return new DateTimeValue(callNative(() => native.temporalDateTimeUtc(date, time)));
  }

  static withOffset(date: DateValue, time: TimeValue, offsetSeconds: number): DateTimeValue {
    return new DateTimeValue(
      callNative(() => native.temporalDateTimeWithOffset(date, time, offsetSeconds)),
    );
  }

  static parse(text: string): DateTimeValue {
    return new DateTimeValue(
      callNative(() => native.temporalDateTimeParse(requireText(text, 'datetime text'))),
    );
  }

  equals(other: DateTimeValue): boolean {
    return (
      this.isUtc === other.isUtc && this.date.equals(other.date) && this.time.equals(other.time)
    );
  }

  toString(): string {
    return this.#text;
  }
}

export type SemanticScalar =
  null | boolean | bigint | Binary64 | string | DateValue | TimeValue | DateTimeValue;

export type SemanticValue = SemanticScalar | readonly SemanticValue[] | ObjectValue;

const encoder = new TextEncoder();

const compareBytes = (left: Uint8Array, right: Uint8Array): number => {
  const shared = Math.min(left.length, right.length);
  for (let index = 0; index < shared; index += 1) {
    const difference = (left[index] ?? 0) - (right[index] ?? 0);
    if (difference !== 0) {
      return difference;
    }
  }
  return left.length - right.length;
};

export class ObjectValue implements Iterable<readonly [string, SemanticValue]> {
  readonly #entries: readonly (readonly [string, SemanticValue])[];
  readonly #lookup: ReadonlyMap<string, SemanticValue>;

  constructor(members: Iterable<readonly [string, SemanticValue]> = []) {
    const keyed = Array.from(members, (member) => {
      const [key, value] = member;
      return { key: requireText(key, 'object key'), bytes: encoder.encode(key), value };
    }).sort((left, right) => compareBytes(left.bytes, right.bytes));
    const lookup = new Map(keyed.map(({ key, value }) => [key, value]));
    if (lookup.size !== keyed.length) {
      throw new ValueError('duplicate object key');
    }
    this.#entries = Object.freeze(
      keyed.map(({ key, value }) => Object.freeze([key, value] as const)),
    );
    this.#lookup = lookup;
    Object.freeze(this);
  }

  get size(): number {
    return this.#entries.length;
  }

  get(key: string): SemanticValue | undefined {
    return this.#lookup.get(key);
  }

  has(key: string): boolean {
    return this.#lookup.has(key);
  }

  entries(): readonly (readonly [string, SemanticValue])[] {
    return this.#entries;
  }

  [Symbol.iterator](): Iterator<readonly [string, SemanticValue]> {
    return this.#entries[Symbol.iterator]();
  }
}

export function encodeValue(value: SemanticValue): Uint8Array {
  return callNative(() => native.valuesEncode(toNative(value)));
}

export function decodeValue(bytes: Uint8Array): SemanticValue {
  return fromNative(callNative(() => native.valuesDecode(bytes)));
}

export function valuesEqual(a: SemanticValue, b: SemanticValue): boolean {
  return callNative(() => native.valuesEqual(toNative(a), toNative(b)));
}
