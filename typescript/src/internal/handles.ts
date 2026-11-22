import { ValueError } from '../errors.js';

export interface Brand {
  [Symbol.hasInstance](value: unknown): boolean;
}

type Class<T> = abstract new (...args: never[]) => T;

const handles = new WeakMap<object, unknown>();

export function attachHandle<T extends object>(owner: T, handle: unknown): T {
  handles.set(owner, handle);
  return owner;
}

export function handleOf(owner: object): unknown {
  return handles.get(owner);
}

const brandedHandle = (owner: unknown, type: Brand): unknown =>
  typeof owner === 'object' && owner !== null && owner instanceof type
    ? handles.get(owner)
    : undefined;

export function hasHandle<H>(owner: unknown, type: Brand, nativeType: Class<H>): boolean {
  return brandedHandle(owner, type) instanceof nativeType;
}

export function requireHandle<H>(
  owner: unknown,
  type: Brand,
  nativeType: Class<H>,
  message: string,
): H {
  const handle = brandedHandle(owner, type);
  if (!(handle instanceof nativeType)) {
    throw new ValueError(message);
  }
  return handle;
}
