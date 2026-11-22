import type { Bundle } from './bundle.js';
import { ValueError } from './errors.js';
import {
  lockTimeout,
  runMutation,
  runReadOnly,
  type CallOptions,
  type LockOptions,
} from './internal/abort.js';
import { awaitNative } from './internal/call.js';
import { handleOf } from './internal/handles.js';
import { requireText } from './internal/text.js';
import { bundleHandle } from './lifecycle.js';
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
import { Target } from './targets.js';

export interface SkillOptions extends LockOptions {
  readonly replaceModified?: boolean;
}

const targetHandle = (target: Target): NativeTarget => {
  const handle = handleOf(target);
  if (!(target instanceof Target) || !(handle instanceof native.NativeTarget)) {
    throw new ValueError('target must be a Target');
  }
  return handle;
};

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
        requireText(bundleId, 'bundle id'),
        targetHandles(targets),
        storeHandle(store),
        skillOptions(options),
        token,
      ),
    );
    return targetResultsFromWire(wire.items, targets);
  });
}
