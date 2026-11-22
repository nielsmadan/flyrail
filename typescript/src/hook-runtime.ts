import { directoryEntry, fileEntry, type BundleEntry } from './bundle.js';
import type { HookShell } from './enums.js';
import { callNative } from './internal/call.js';
import { entryFromNative } from './internal/entries.js';
import { attachHandle } from './internal/handles.js';
import { optionalAbsolute } from './internal/paths.js';
import type { NativeHookRuntime } from './native/binding.js';
import { native } from './native/loader.js';

export interface HookRuntimeOptions {
  readonly node?: string | null;
  readonly shell?: HookShell | null;
}

const copyEntry = (entry: BundleEntry): BundleEntry =>
  entry.data === null
    ? directoryEntry(entry.path)
    : fileEntry(entry.path, entry.data, { executable: entry.executable });

export class HookRuntime {
  readonly node: string | null;
  readonly shell: HookShell | null;
  readonly #resources: readonly BundleEntry[];

  constructor(options?: HookRuntimeOptions) {
    const handle: NativeHookRuntime = callNative(() =>
      native.hookRuntimeNew(
        optionalAbsolute(options?.node, 'hook runtime node') ?? undefined,
        options?.shell ?? undefined,
      ),
    );
    const snapshot = handle.snapshot();
    this.node = snapshot.node ?? null;
    this.shell = (snapshot.shell ?? null) as HookShell | null;
    this.#resources = Object.freeze(snapshot.resources.map(entryFromNative));
    attachHandle(this, handle);
    Object.freeze(this);
  }

  get resources(): readonly BundleEntry[] {
    return Object.freeze(this.#resources.map(copyEntry));
  }
}
