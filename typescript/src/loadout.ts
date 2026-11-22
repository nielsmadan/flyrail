import type { Agent, Platform } from './enums.js';
import { FlyrailError, ValueError } from './errors.js';
import { adoptInstallationTarget, type InstallationTarget } from './installation.js';
import {
  lockTimeout,
  runMutation,
  runReadOnly,
  type CallOptions,
  type LockOptions,
} from './internal/abort.js';
import { awaitNative, callNative } from './internal/call.js';
import { attachHandle, handleOf } from './internal/handles.js';
import { fromPairObjects, toPairObjects } from './internal/maps.js';
import { buildModel, modelHandle, type LoadoutLocationWire } from './internal/models.js';
import { optionalAbsolute, requireAbsolute } from './internal/paths.js';
import { requireText } from './internal/text.js';
import type { Bundle } from './bundle.js';
import { HookRuntime } from './hook-runtime.js';
import {
  bundleHandle,
  installationResultFromWire,
  observationFromWire,
  previewFromHandle,
  type InstallationObservation,
  type InstallationResult,
  type LifecyclePreview,
} from './lifecycle.js';
import type {
  JsLoadoutDiagnostic,
  JsLoadoutSetup,
  JsLoadoutSource,
  NativeHookRuntime,
  NativeLoadoutDestination,
  NativeLoadoutPreview,
  NativeLoadoutSetup,
} from './native/binding.js';
import { native } from './native/loader.js';
import { storeHandle, type StateStore } from './stores.js';

export class LoadoutDiagnostic extends FlyrailError {
  override name = 'LoadoutDiagnostic';
  readonly code: string;
  readonly path: string;

  constructor(code: string, message: string, path: string) {
    super(message);
    this.code = code;
    this.path = path;
  }
}

export interface LoadoutSource {
  readonly name: string;
  readonly path: string;
  readonly uses: readonly string[];
  readonly declaredIn: string;
  readonly overrides: Readonly<Record<string, readonly string[]>>;
}

export interface LoadoutHarness {
  readonly agent: Agent;
  readonly slices: readonly string[];
}

export interface LoadoutSelector {
  readonly agent: Agent;
  readonly category: string;
  readonly manifest: string;
  readonly key: readonly string[];
  readonly entries: readonly string[];
  readonly substitutions: Readonly<Record<string, string>>;
}

export interface LoadoutImpact {
  readonly category: string;
  readonly path: string;
  readonly harnesses: readonly Agent[];
  readonly shared: boolean;
}

export interface LoadoutResult {
  readonly location: LoadoutLocation;
  readonly lifecycle: InstallationResult;
  readonly syncRequired: boolean;
}

export interface LoadoutOptions extends CallOptions {
  readonly platform: Platform;
  readonly hookRuntime?: HookRuntime | null;
  readonly executionRoot?: string | null;
  readonly replaceModified?: boolean;
}

export interface DiscoverLoadoutOptions extends CallOptions {
  readonly home: string;
  readonly configHome?: string | null;
  readonly environment: Readonly<Record<string, string>>;
}

const diagnosticFromWire = (wire: JsLoadoutDiagnostic): LoadoutDiagnostic =>
  new LoadoutDiagnostic(wire.code, wire.message, wire.path);

const strings = (items: readonly string[]): readonly string[] => Object.freeze([...items]);

const sourceFromWire = (wire: JsLoadoutSource): LoadoutSource =>
  Object.freeze({
    name: wire.name,
    path: wire.path,
    uses: strings(wire.uses),
    declaredIn: wire.declaredIn,
    overrides: Object.freeze(
      Object.fromEntries(wire.overrides.map(({ key, values }) => [key, strings(values)])),
    ),
  });

const agentNames = (agents: readonly Agent[]): string[] => {
  if (!Array.isArray(agents)) {
    throw new ValueError('harnesses must be an array of agents');
  }
  return (agents as readonly string[]).map((agent) => requireText(agent, 'harness'));
};

export class LoadoutLocation {
  readonly root: string;
  readonly profile: string;
  readonly sourceName: string;
  readonly sourcePath: string;

  constructor(root: string, profile: string, sourceName: string, sourcePath: string) {
    const wire: LoadoutLocationWire = {
      kind: 'loadout-location',
      root: requireAbsolute(root, 'Loadout root'),
      profile: requireText(profile, 'Loadout profile'),
      sourceName: requireText(sourceName, 'Loadout source name'),
      sourcePath: requireAbsolute(sourcePath, 'Loadout source path'),
    };
    const built = buildModel<LoadoutLocationWire>(wire);
    this.root = built.snapshot.root;
    this.profile = built.snapshot.profile;
    this.sourceName = built.snapshot.sourceName;
    this.sourcePath = built.snapshot.sourcePath;
    attachHandle(this, built.handle);
    Object.freeze(this);
  }

  installation(store: StateStore): InstallationTarget {
    return adoptInstallationTarget(
      store,
      callNative(() =>
        native.loadoutLocationInstallation(locationHandle(this), storeHandle(store)),
      ),
    );
  }
}

const locationHandle = (location: LoadoutLocation) => {
  const handle = modelHandle(location);
  if (!(location instanceof LoadoutLocation) || !(handle instanceof native.NativeModel)) {
    throw new ValueError('location must be a LoadoutLocation');
  }
  return handle;
};

const locationFromWire = (wire: {
  readonly root: string;
  readonly profile: string;
  readonly sourceName: string;
  readonly sourcePath: string;
}): LoadoutLocation =>
  new LoadoutLocation(wire.root, wire.profile, wire.sourceName, wire.sourcePath);

let createDestination: (handle: NativeLoadoutDestination) => LoadoutDestination;

export class LoadoutDestination {
  readonly location: LoadoutLocation;
  readonly source: LoadoutSource;
  readonly harnesses: readonly Agent[];

  private constructor(handle: NativeLoadoutDestination) {
    const wire = callNative(() => handle.snapshot());
    this.location = locationFromWire(wire.location);
    this.source = sourceFromWire(wire.source);
    this.harnesses = Object.freeze([...wire.harnesses] as Agent[]);
    attachHandle(this, handle);
    Object.freeze(this);
  }

  static {
    createDestination = (handle) => new LoadoutDestination(handle);
  }
}

const destinationHandle = (destination: LoadoutDestination): NativeLoadoutDestination => {
  const handle = handleOf(destination);
  if (
    !(destination instanceof LoadoutDestination) ||
    !(handle instanceof native.NativeLoadoutDestination)
  ) {
    throw new ValueError('destination must be a LoadoutDestination');
  }
  return handle;
};

let createSetup: (handle: NativeLoadoutSetup) => LoadoutSetup;

export class LoadoutSetup {
  readonly registry: string;
  readonly root: string;
  readonly profile: string;
  readonly profileChain: readonly string[];
  readonly sources: readonly LoadoutSource[];
  readonly harnesses: readonly LoadoutHarness[];
  readonly selectors: readonly LoadoutSelector[];

  private constructor(handle: NativeLoadoutSetup) {
    const wire: JsLoadoutSetup = handle.snapshot();
    this.registry = wire.registry;
    this.root = wire.root;
    this.profile = wire.profile;
    this.profileChain = strings(wire.profileChain);
    this.sources = Object.freeze(wire.sources.map(sourceFromWire));
    this.harnesses = Object.freeze(
      wire.harnesses.map((harness) =>
        Object.freeze({ agent: harness.agent as Agent, slices: strings(harness.slices) }),
      ),
    );
    this.selectors = Object.freeze(
      wire.selectors.map((selector) =>
        Object.freeze({
          agent: selector.agent as Agent,
          category: selector.category,
          manifest: selector.manifest,
          key: strings(selector.key),
          entries: strings(selector.entries),
          substitutions: fromPairObjects(selector.substitutions),
        }),
      ),
    );
    attachHandle(this, handle);
    Object.freeze(this);
  }

  eligibleSources(bundle: Bundle, harnesses: readonly Agent[]): readonly LoadoutSource[] {
    const sources = callNative(() =>
      setupHandle(this).eligibleSources(bundleHandle(bundle), agentNames(harnesses)),
    );
    return Object.freeze(sources.map(sourceFromWire));
  }

  select(
    name: string,
    harnesses: readonly Agent[],
    options?: { readonly acceptSharedScope?: boolean },
  ): LoadoutDestination {
    return createDestination(
      callNative(() =>
        setupHandle(this).select(
          requireText(name, 'Loadout source name'),
          agentNames(harnesses),
          options?.acceptSharedScope ?? false,
        ),
      ),
    );
  }

  static {
    createSetup = (handle) => new LoadoutSetup(handle);
  }
}

const setupHandle = (setup: LoadoutSetup): NativeLoadoutSetup => {
  const handle = handleOf(setup);
  if (!(setup instanceof LoadoutSetup) || !(handle instanceof native.NativeLoadoutSetup)) {
    throw new ValueError('setup must be a LoadoutSetup');
  }
  return handle;
};

export type LoadoutDiscovery =
  | { readonly status: 'absent'; readonly registry: string }
  | { readonly status: 'available'; readonly registry: string; readonly setup: LoadoutSetup }
  | {
      readonly status: 'invalid';
      readonly registry: string;
      readonly diagnostic: LoadoutDiagnostic;
    };

export function discoverLoadout(options: DiscoverLoadoutOptions): Promise<LoadoutDiscovery> {
  return runReadOnly(options.signal, async () => {
    const home = requireAbsolute(options.home, 'home');
    const configHome = optionalAbsolute(options.configHome, 'config home');
    const environment: unknown = options.environment;
    if (typeof environment !== 'object' || environment === null) {
      throw new ValueError('environment must be an object');
    }
    const wire = await awaitNative(
      native.loadoutDiscover(
        home,
        configHome ?? undefined,
        toPairObjects(options.environment).map(({ key, value }) => ({
          key: requireText(key, 'environment name'),
          value: requireText(value, 'environment value'),
        })),
      ),
    );
    switch (wire.status) {
      case 'absent':
        return Object.freeze({ status: 'absent', registry: wire.registry } as const);
      case 'available':
        if (wire.setup === undefined) {
          throw new ValueError('Loadout discovery returned no setup');
        }
        return Object.freeze({
          status: 'available',
          registry: wire.registry,
          setup: createSetup(wire.setup),
        } as const);
      case 'invalid':
        if (wire.diagnostic === undefined) {
          throw new ValueError('Loadout discovery returned no diagnostic');
        }
        return Object.freeze({
          status: 'invalid',
          registry: wire.registry,
          diagnostic: diagnosticFromWire(wire.diagnostic),
        } as const);
    }
  });
}

interface PreviewParts {
  readonly handle: NativeLoadoutPreview;
  readonly bundle: Bundle | null;
  readonly store: StateStore;
}

let createPreview: (parts: PreviewParts) => LoadoutPreview;

export class LoadoutPreview {
  readonly location: LoadoutLocation;
  readonly lifecycle: LifecyclePreview;
  readonly impact: readonly LoadoutImpact[];
  readonly diagnostic: LoadoutDiagnostic | null;
  readonly applicable: boolean;
  readonly scopeNotice: string;

  private constructor(parts: PreviewParts) {
    const wire = parts.handle.snapshot();
    this.location = locationFromWire(wire.location);
    this.lifecycle = previewFromHandle(
      parts.handle.lifecycle(),
      parts.bundle,
      null,
      this.location.installation(parts.store),
    );
    this.impact = Object.freeze(
      wire.impact.map((impact) =>
        Object.freeze({
          category: impact.category,
          path: impact.path,
          harnesses: Object.freeze([...impact.harnesses] as Agent[]),
          shared: impact.shared,
        }),
      ),
    );
    this.diagnostic = wire.diagnostic === undefined ? null : diagnosticFromWire(wire.diagnostic);
    this.applicable = wire.applicable;
    this.scopeNotice = wire.scopeNotice;
    attachHandle(this, parts.handle);
    Object.freeze(this);
  }

  static {
    createPreview = (parts) => new LoadoutPreview(parts);
  }
}

const loadoutPreviewHandle = (preview: LoadoutPreview): NativeLoadoutPreview => {
  const handle = handleOf(preview);
  if (!(preview instanceof LoadoutPreview) || !(handle instanceof native.NativeLoadoutPreview)) {
    throw new ValueError('preview must be a LoadoutPreview');
  }
  return handle;
};

const runtimeHandle = (runtime: HookRuntime | null | undefined): NativeHookRuntime | undefined => {
  if (runtime === null || runtime === undefined) {
    return undefined;
  }
  const handle = handleOf(runtime);
  if (!(runtime instanceof HookRuntime) || !(handle instanceof native.NativeHookRuntime)) {
    throw new ValueError('hook runtime must be a HookRuntime');
  }
  return handle;
};

const bundleId = (id: string): string => requireText(id, 'bundle id');

export function previewLoadout(
  bundle: Bundle,
  destination: LoadoutDestination,
  store: StateStore,
  options: LoadoutOptions,
): Promise<LoadoutPreview> {
  return runReadOnly(options.signal, async () => {
    const executionRoot = optionalAbsolute(options.executionRoot, 'execution root');
    const handle = await awaitNative(
      native.loadoutPreview(
        bundleHandle(bundle),
        destinationHandle(destination),
        storeHandle(store),
        runtimeHandle(options.hookRuntime),
        {
          platform: requireText(options.platform, 'platform'),
          ...(executionRoot === null ? {} : { executionRoot }),
          replaceModified: options.replaceModified ?? false,
        },
      ),
    );
    return createPreview({ handle, bundle, store });
  });
}

export function previewLoadoutRemoval(
  id: string,
  location: LoadoutLocation,
  store: StateStore,
  options?: CallOptions & { readonly replaceModified?: boolean },
): Promise<LoadoutPreview> {
  return runReadOnly(options?.signal, async () => {
    const handle = await awaitNative(
      native.loadoutPreviewRemoval(
        bundleId(id),
        locationHandle(location),
        storeHandle(store),
        options?.replaceModified ?? false,
      ),
    );
    return createPreview({ handle, bundle: null, store });
  });
}

export function applyLoadout(
  preview: LoadoutPreview,
  options?: LockOptions,
): Promise<LoadoutResult> {
  return runMutation(options?.signal, async (token) => {
    const wire = await awaitNative(
      native.loadoutApply(
        loadoutPreviewHandle(preview),
        { lockTimeoutMs: lockTimeout(options) },
        token,
      ),
    );
    return Object.freeze({
      location: preview.location,
      lifecycle: installationResultFromWire(wire.lifecycle, preview.lifecycle.target),
      syncRequired: wire.syncRequired,
    });
  });
}

export function inspectLoadout(
  id: string,
  location: LoadoutLocation,
  store: StateStore,
  options?: CallOptions,
): Promise<InstallationObservation> {
  return runReadOnly(options?.signal, async () =>
    observationFromWire(
      await awaitNative(
        native.loadoutInspect(bundleId(id), locationHandle(location), storeHandle(store)),
      ),
      location.installation(store),
    ),
  );
}

export function recoverLoadout(
  id: string,
  location: LoadoutLocation,
  store: StateStore,
  options?: LockOptions,
): Promise<LoadoutResult> {
  return runMutation(options?.signal, async (token) => {
    const wire = await awaitNative(
      native.loadoutRecover(
        bundleId(id),
        locationHandle(location),
        storeHandle(store),
        { lockTimeoutMs: lockTimeout(options) },
        token,
      ),
    );
    return Object.freeze({
      location,
      lifecycle: installationResultFromWire(wire.lifecycle, location.installation(store)),
      syncRequired: wire.syncRequired,
    });
  });
}
