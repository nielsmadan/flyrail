import type { Bundle } from './bundle.js';
import { ValueError } from './errors.js';
import { lockTimeout, runMutation, runReadOnly } from './internal/abort.js';
import type { CallOptions, LockOptions } from './options.js';
import { awaitNative } from './internal/call.js';
import { bundleHandle, targetHandle } from './internal/guards.js';
import { requireBundleId } from './internal/text.js';
import type { NativeTarget } from './native/binding.js';
import { native } from './native/loader.js';
import {
  inspectionsFromWire,
  targetResultsFromWire,
  type TargetInspection,
  type TargetResult,
} from './observations.js';
import { adoptRenderedBundle, type RenderedBundle } from './rendered.js';
import { storeHandle, type StateStore } from './stores.js';
import type { Target } from './targets.js';

export interface SkillOptions extends LockOptions {
  readonly replaceModified?: boolean;
}

const targetHandles = (targets: readonly Target[]): NativeTarget[] => {
  if (!Array.isArray(targets)) {
    throw new ValueError('targets must be an array of Target');
  }
  return targets.map(targetHandle);
};

const skillOptions = (
  options: SkillOptions | undefined,
): {
  replaceModified: boolean;
  lockTimeoutMs: number;
} => ({
  replaceModified: options?.replaceModified ?? false,
  lockTimeoutMs: lockTimeout(options),
});

export function renderSkills(
  bundle: Bundle,
  target: Target,
  options?: CallOptions,
): Promise<RenderedBundle> {
  return runReadOnly(options?.signal, async () =>
    adoptRenderedBundle(
      await awaitNative(native.skillsRender(bundleHandle(bundle), targetHandle(target))),
    ),
  );
}

export function inspect(
  bundle: Bundle,
  targets: readonly Target[],
  store: StateStore,
  options?: CallOptions,
): Promise<readonly TargetInspection[]> {
  return runReadOnly(options?.signal, async () => {
    const wire = await awaitNative(
      native.skillsInspect(bundleHandle(bundle), targetHandles(targets), storeHandle(store)),
    );
    return inspectionsFromWire(wire.items, targets);
  });
}

export function install(
  bundle: Bundle,
  targets: readonly Target[],
  store: StateStore,
  options?: LockOptions,
): Promise<readonly TargetResult[]> {
  return runMutation(options?.signal, async (token) => {
    const wire = await awaitNative(
      native.skillsInstall(
        bundleHandle(bundle),
        targetHandles(targets),
        storeHandle(store),
        skillOptions(options),
        token,
      ),
    );
    return targetResultsFromWire(wire.items, targets);
  });
}

export function update(
  bundle: Bundle,
  targets: readonly Target[],
  store: StateStore,
  options?: SkillOptions,
): Promise<readonly TargetResult[]> {
  return runMutation(options?.signal, async (token) => {
    const wire = await awaitNative(
      native.skillsUpdate(
        bundleHandle(bundle),
        targetHandles(targets),
        storeHandle(store),
        skillOptions(options),
        token,
      ),
    );
    return targetResultsFromWire(wire.items, targets);
  });
}

export function uninstall(
  bundleId: string,
  targets: readonly Target[],
  store: StateStore,
  options?: SkillOptions,
): Promise<readonly TargetResult[]> {
  return runMutation(options?.signal, async (token) => {
    const wire = await awaitNative(
      native.skillsUninstall(
        requireBundleId(bundleId),
        targetHandles(targets),
        storeHandle(store),
        skillOptions(options),
        token,
      ),
    );
    return targetResultsFromWire(wire.items, targets);
  });
}
