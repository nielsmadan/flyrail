const handles = new WeakMap<object, unknown>();

export function attachHandle<T extends object>(owner: T, handle: unknown): T {
  handles.set(owner, handle);
  return owner;
}

export function handleOf(owner: object): unknown {
  return handles.get(owner);
}
