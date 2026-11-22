import { ValueError } from './errors.js';
import { runMutation, runReadOnly, type CallOptions } from './internal/abort.js';
import { awaitNative, callNative } from './internal/call.js';
import { attachHandle, handleOf } from './internal/handles.js';
import { requireAbsolute } from './internal/paths.js';
import { requireText } from './internal/text.js';
import type { NativeStateStore } from './native/binding.js';
import { native } from './native/loader.js';

const bytesOf = (value: Uint8Array, label: string): Uint8Array => {
  if (!(value instanceof Uint8Array)) {
    throw new ValueError(`${label} must be a Uint8Array`);
  }
  return new Uint8Array(value);
};

const read = (store: StateStore, key: string, options?: CallOptions): Promise<Uint8Array | null> =>
  runReadOnly(options?.signal, () =>
    awaitNative(storeHandle(store).read(requireText(key, 'state key'))),
  );

const write = (
  store: StateStore,
  key: string,
  value: Uint8Array,
  expected: Uint8Array | null,
  options?: CallOptions,
): Promise<boolean> =>
  runMutation(options?.signal, () =>
    awaitNative(
      storeHandle(store).write(
        requireText(key, 'state key'),
        bytesOf(value, 'value'),
        expected === null ? null : bytesOf(expected, 'expected value'),
      ),
    ),
  );

const remove = (
  store: StateStore,
  key: string,
  expected: Uint8Array,
  options?: CallOptions,
): Promise<boolean> =>
  runMutation(options?.signal, () =>
    awaitNative(
      storeHandle(store).delete(requireText(key, 'state key'), bytesOf(expected, 'expected value')),
    ),
  );

const keys = (
  store: StateStore,
  prefix: string,
  options?: CallOptions,
): Promise<readonly string[]> =>
  runReadOnly(options?.signal, async () =>
    Object.freeze(await awaitNative(storeHandle(store).keys(requireText(prefix, 'key prefix')))),
  );

export class DirectoryStore {
  readonly location: string;
  readonly stagingDirectory: string;

  private constructor(handle: NativeStateStore) {
    const snapshot = handle.snapshot();
    this.location = snapshot.location ?? '';
    this.stagingDirectory = snapshot.stagingDirectory ?? '';
    attachHandle(this, handle);
    Object.freeze(this);
  }

  static open(path: string, options?: CallOptions): Promise<DirectoryStore> {
    return runReadOnly(options?.signal, () =>
      Promise.resolve(
        new DirectoryStore(
          callNative(() => native.storeOpenDirectory(requireAbsolute(path, 'store path'))),
        ),
      ),
    );
  }

  read(key: string, options?: CallOptions): Promise<Uint8Array | null> {
    return read(this, key, options);
  }

  write(
    key: string,
    value: Uint8Array,
    expected: Uint8Array | null,
    options?: CallOptions,
  ): Promise<boolean> {
    return write(this, key, value, expected, options);
  }

  delete(key: string, expected: Uint8Array, options?: CallOptions): Promise<boolean> {
    return remove(this, key, expected, options);
  }

  keys(prefix: string, options?: CallOptions): Promise<readonly string[]> {
    return keys(this, prefix, options);
  }
}

export class MemoryStore {
  readonly location: null = null;
  readonly stagingDirectory: null = null;

  constructor() {
    attachHandle(
      this,
      callNative(() => native.storeNewMemory()),
    );
    Object.freeze(this);
  }

  read(key: string, options?: CallOptions): Promise<Uint8Array | null> {
    return read(this, key, options);
  }

  write(
    key: string,
    value: Uint8Array,
    expected: Uint8Array | null,
    options?: CallOptions,
  ): Promise<boolean> {
    return write(this, key, value, expected, options);
  }

  delete(key: string, expected: Uint8Array, options?: CallOptions): Promise<boolean> {
    return remove(this, key, expected, options);
  }

  keys(prefix: string, options?: CallOptions): Promise<readonly string[]> {
    return keys(this, prefix, options);
  }
}

export type StateStore = DirectoryStore | MemoryStore;

export function storeHandle(store: StateStore): NativeStateStore {
  const handle = handleOf(store);
  if (
    !(store instanceof DirectoryStore || store instanceof MemoryStore) ||
    !(handle instanceof native.NativeStateStore)
  ) {
    throw new ValueError('store must be a DirectoryStore or MemoryStore');
  }
  return handle;
}

export function validateKey(key: string): void {
  callNative(() => native.storeValidateKey(requireText(key, 'state key')));
}

export function validateLockName(name: string): void {
  callNative(() => native.storeValidateLockName(requireText(name, 'lock name')));
}
