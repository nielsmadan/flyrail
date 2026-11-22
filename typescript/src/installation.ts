import { ValueError } from './errors.js';
import { callNative } from './internal/call.js';
import { attachHandle, handleOf } from './internal/handles.js';
import { fromPairObjects, toPairObjects, type PairObject } from './internal/maps.js';
import { requireText } from './internal/text.js';
import type { NativeInstallationTarget } from './native/binding.js';
import { native } from './native/loader.js';
import { storeHandle, type StateStore } from './stores.js';

export const contextPairs = (
  record: Readonly<Record<string, string>> | null | undefined,
  label: string,
): PairObject[] =>
  toPairObjects(record).map(({ key, value }) => ({
    key: requireText(key, `${label} key`),
    value: requireText(value, `${label} value`),
  }));

let adoptable: NativeInstallationTarget | null = null;

export class InstallationTarget {
  readonly store: StateStore;
  readonly context: Readonly<Record<string, string>>;
  readonly routingContext: Readonly<Record<string, string>>;
  readonly contextDigest: string;

  constructor(
    store: StateStore,
    context?: Readonly<Record<string, string>> | null,
    routingContext?: Readonly<Record<string, string>> | null,
  ) {
    const handle: NativeInstallationTarget =
      adoptable ??
      callNative(() =>
        native.installationTargetNew(
          storeHandle(store),
          contextPairs(context, 'context'),
          contextPairs(routingContext, 'routing context'),
        ),
      );
    adoptable = null;
    const snapshot = handle.snapshot();
    this.store = store;
    this.context = fromPairObjects(snapshot.context);
    this.routingContext = fromPairObjects(snapshot.routingContext);
    this.contextDigest = snapshot.contextDigest;
    attachHandle(this, handle);
    Object.freeze(this);
  }

  equals(other: InstallationTarget): boolean {
    const otherHandle = handleOf(other);
    return (
      otherHandle instanceof native.NativeInstallationTarget &&
      installationHandle(this).equals(otherHandle)
    );
  }
}

export function adoptInstallationTarget(
  store: StateStore,
  handle: NativeInstallationTarget,
): InstallationTarget {
  adoptable = handle;
  try {
    return new InstallationTarget(store);
  } finally {
    adoptable = null;
  }
}

export function installationHandle(target: InstallationTarget): NativeInstallationTarget {
  const handle = handleOf(target);
  if (
    !(target instanceof InstallationTarget) ||
    !(handle instanceof native.NativeInstallationTarget)
  ) {
    throw new ValueError('target must be an InstallationTarget');
  }
  return handle;
}
