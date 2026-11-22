import { ValueError } from '../errors.js';
import { DateTimeValue, DateValue, ObjectValue, TimeValue, type SemanticValue } from '../values.js';
import { requireText } from './text.js';

export interface NativeDate {
  readonly year: number;
  readonly month: number;
  readonly day: number;
}

export interface NativeTime {
  readonly hour: number;
  readonly minute: number;
  readonly second: number;
  readonly microsecond: number;
}

export type NativeValue =
  | { readonly t: 'null' }
  | { readonly t: 'bool'; readonly v: boolean }
  | { readonly t: 'int'; readonly v: bigint }
  | { readonly t: 'float'; readonly v: number }
  | { readonly t: 'text'; readonly v: string }
  | ({ readonly t: 'date' } & NativeDate)
  | ({ readonly t: 'time' } & NativeTime)
  | {
      readonly t: 'datetime';
      readonly date: NativeDate;
      readonly time: NativeTime;
      readonly utc: boolean;
    }
  | { readonly t: 'array'; readonly items: readonly NativeValue[] }
  | { readonly t: 'object'; readonly members: readonly (readonly [string, NativeValue])[] };

const MAX_DEPTH = 512;
const INT64_MIN = -(2n ** 63n);
const INT64_MAX = 2n ** 63n - 1n;

type Container = 'array' | 'object';

type Opened<In, Out> =
  | { readonly kind: 'leaf'; readonly value: Out }
  | {
      readonly kind: Container;
      readonly keys: readonly string[];
      readonly children: readonly In[];
    };

interface Frame<In, Out> {
  readonly kind: Container;
  readonly keys: readonly string[];
  readonly children: readonly In[];
  readonly outs: Out[];
  index: number;
}

function walk<In, Out>(
  root: In,
  open: (value: In) => Opened<In, Out>,
  close: (kind: Container, keys: readonly string[], outs: readonly Out[]) => Out,
): Out {
  const stack: Frame<In, Out>[] = [];
  let pending: { readonly value: In } | null = { value: root };
  for (;;) {
    let done: { readonly value: Out } | null = null;
    if (pending !== null) {
      const opened = open(pending.value);
      pending = null;
      if (opened.kind === 'leaf') {
        done = { value: opened.value };
      } else {
        if (stack.length >= MAX_DEPTH) {
          throw new ValueError('value nesting is too deep');
        }
        stack.push({ ...opened, outs: [], index: 0 });
      }
    } else {
      const top = stack.at(-1);
      if (top === undefined) {
        throw new ValueError('value conversion lost its root');
      }
      if (top.index < top.children.length) {
        pending = { value: top.children[top.index] as In };
        top.index += 1;
      } else {
        stack.pop();
        done = { value: close(top.kind, top.keys, top.outs) };
      }
    }
    if (done !== null) {
      const parent = stack.at(-1);
      if (parent === undefined) {
        return done.value;
      }
      parent.outs.push(done.value);
    }
  }
}

const isArray = (value: SemanticValue): value is readonly SemanticValue[] => Array.isArray(value);

const dateOf = (value: DateValue): NativeDate => ({
  year: value.year,
  month: value.month,
  day: value.day,
});

const timeOf = (value: TimeValue): NativeTime => ({
  hour: value.hour,
  minute: value.minute,
  second: value.second,
  microsecond: value.microsecond,
});

function scalarToNative(value: SemanticValue): NativeValue {
  if (value === null) {
    return { t: 'null' };
  }
  switch (typeof value) {
    case 'boolean':
      return { t: 'bool', v: value };
    case 'bigint':
      if (value < INT64_MIN || value > INT64_MAX) {
        throw new ValueError('integers must fit signed 64 bits');
      }
      return { t: 'int', v: value };
    case 'number':
      if (!Number.isFinite(value)) {
        throw new ValueError('floats must be finite');
      }
      return { t: 'float', v: value };
    case 'string':
      return { t: 'text', v: requireText(value, 'text value') };
    default:
      break;
  }
  if (value instanceof DateValue) {
    return { t: 'date', ...dateOf(value) };
  }
  if (value instanceof TimeValue) {
    return { t: 'time', ...timeOf(value) };
  }
  if (value instanceof DateTimeValue) {
    return { t: 'datetime', date: dateOf(value.date), time: timeOf(value.time), utc: value.isUtc };
  }
  throw new ValueError('value is not a semantic value');
}

function openSemantic(value: SemanticValue): Opened<SemanticValue, NativeValue> {
  if (isArray(value)) {
    return { kind: 'array', keys: [], children: value };
  }
  if (value instanceof ObjectValue) {
    const entries = value.entries();
    return {
      kind: 'object',
      keys: entries.map(([key]) => key),
      children: entries.map(([, member]) => member),
    };
  }
  return { kind: 'leaf', value: scalarToNative(value) };
}

function closeNative(
  kind: Container,
  keys: readonly string[],
  outs: readonly NativeValue[],
): NativeValue {
  return kind === 'array'
    ? { t: 'array', items: outs }
    : { t: 'object', members: outs.map((out, index) => [keys[index] ?? '', out] as const) };
}

function scalarFromNative(value: NativeValue): SemanticValue {
  switch (value.t) {
    case 'null':
      return null;
    case 'bool':
    case 'int':
    case 'float':
    case 'text':
      return value.v;
    case 'date':
      return new DateValue(value.year, value.month, value.day);
    case 'time':
      return new TimeValue(value.hour, value.minute, value.second, value.microsecond);
    case 'datetime': {
      const date = new DateValue(value.date.year, value.date.month, value.date.day);
      const time = new TimeValue(
        value.time.hour,
        value.time.minute,
        value.time.second,
        value.time.microsecond,
      );
      return value.utc ? DateTimeValue.utc(date, time) : DateTimeValue.local(date, time);
    }
    default:
      throw new ValueError('native value has an unknown tag');
  }
}

function openNative(value: NativeValue): Opened<NativeValue, SemanticValue> {
  switch (value.t) {
    case 'array':
      return { kind: 'array', keys: [], children: value.items };
    case 'object':
      return {
        kind: 'object',
        keys: value.members.map(([key]) => key),
        children: value.members.map(([, member]) => member),
      };
    default:
      return { kind: 'leaf', value: scalarFromNative(value) };
  }
}

function closeSemantic(
  kind: Container,
  keys: readonly string[],
  outs: readonly SemanticValue[],
): SemanticValue {
  return kind === 'array'
    ? Object.freeze([...outs])
    : new ObjectValue(outs.map((out, index) => [keys[index] ?? '', out] as const));
}

export function toNative(value: SemanticValue): NativeValue {
  return walk(value, openSemantic, closeNative);
}

export function fromNative(value: NativeValue): SemanticValue {
  return walk(value, openNative, closeSemantic);
}
