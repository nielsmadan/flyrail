import type { Bundle } from './bundle.js';
import type { Acquisition, EditStatus, OperationStatus, ResourceKind } from './enums.js';
import type { TargetError } from './errors.js';
import { installationHandle, type InstallationTarget } from './installation.js';
import { lockTimeout, runMutation, runReadOnly, type NativeCancelToken } from './internal/abort.js';
import type { CallOptions, LockOptions } from './options.js';
import { awaitNative, callNative, type NativeResult } from './internal/call.js';
import { bundleHandle, renderedHandle } from './internal/guards.js';
import { attachHandle, handleOf, requireHandle } from './internal/handles.js';
import type {
  ClaimObservationWire,
  InstallationObservationWire,
  InstallationResultWire,
  PreviewWire,
  ResourcePlanWire,
  ResourceRefWire,
  ResourceResultWire,
  ResourceObservationWire,
} from './internal/models.js';
import { frozenCopy, targetErrorFromWire } from './internal/results.js';
import { requireBundleId } from './internal/text.js';
import type { JsLifecycleOptions, NativeLifecyclePreview } from './native/binding.js';
import { native } from './native/loader.js';
import {
  adoptRenderedBundle,
  noticeFromWire,
  type Notice,
  type RenderedBundle,
} from './rendered.js';

export interface ResourceRef {
  readonly destination: string;
  readonly kind: ResourceKind;
}

export interface ResourcePlan {
  readonly resource: ResourceRef;
  readonly transactionDirectory: string;
  readonly recoveryPaths: readonly string[];
  readonly requireExisting: boolean;
  readonly error: TargetError | null;
}

export interface ClaimObservation {
  readonly claimId: string;
  readonly bundleId: string;
  readonly artifactId: string;
  readonly version: string;
  readonly bundleDigest: string;
  readonly renderDigest: string;
  readonly status: EditStatus;
}

export interface ResourceObservation {
  readonly destination: string;
  readonly claims: readonly ClaimObservation[];
  readonly revisionDigest: string | null;
  readonly error: TargetError | null;
  readonly recoveryPaths: readonly string[];
}

export interface ResourceResult {
  readonly resource: ResourceRef;
  readonly status: OperationStatus;
  readonly error: TargetError | null;
  readonly recoveryPaths: readonly string[];
}

export interface InstallationResult {
  readonly status: OperationStatus;
  readonly observation: InstallationObservation;
  readonly resources: readonly ResourceResult[];
  readonly notices: readonly Notice[];
  readonly error: TargetError | null;
  readonly succeeded: boolean;
}

export interface PreviewOptions extends CallOptions {
  readonly acquisition?: Acquisition;
  readonly replaceModified?: boolean;
}

export interface SyncOptions extends LockOptions {
  readonly acquisition?: Acquisition;
  readonly replaceModified?: boolean;
}

export interface RemoveOptions extends LockOptions {
  readonly replaceModified?: boolean;
}

const resourceRefFromWire = (wire: ResourceRefWire): ResourceRef =>
  Object.freeze({ destination: wire.destination, kind: wire.kind });

const resourcePlanFromWire = (wire: ResourcePlanWire): ResourcePlan =>
  Object.freeze({
    resource: resourceRefFromWire(wire.resource),
    transactionDirectory: wire.transactionDirectory,
    recoveryPaths: frozenCopy(wire.recoveryPaths),
    requireExisting: wire.requireExisting,
    error: targetErrorFromWire(wire.error),
  });

const claimObservationFromWire = (wire: ClaimObservationWire): ClaimObservation =>
  Object.freeze({
    claimId: wire.claimId,
    bundleId: wire.bundleId,
    artifactId: wire.artifactId,
    version: wire.version,
    bundleDigest: wire.bundleDigest,
    renderDigest: wire.renderDigest,
    status: wire.status,
  });

const resourceObservationFromWire = (wire: ResourceObservationWire): ResourceObservation =>
  Object.freeze({
    destination: wire.destination,
    claims: Object.freeze(wire.claims.map(claimObservationFromWire)),
    revisionDigest: wire.revisionDigest,
    error: targetErrorFromWire(wire.error),
    recoveryPaths: frozenCopy(wire.recoveryPaths),
  });

const resourceResultFromWire = (wire: ResourceResultWire): ResourceResult =>
  Object.freeze({
    resource: resourceRefFromWire(wire.resource),
    status: wire.status,
    error: targetErrorFromWire(wire.error),
    recoveryPaths: frozenCopy(wire.recoveryPaths),
  });

interface PreviewParts {
  readonly handle: NativeLifecyclePreview;
  readonly bundle: Bundle | null;
  readonly rendered: RenderedBundle;
  readonly target: InstallationTarget;
}

let createPreview: (parts: PreviewParts) => LifecyclePreview;

export class LifecyclePreview {
  readonly bundleId: string;
  readonly bundle: Bundle | null;
  readonly rendered: RenderedBundle;
  readonly target: InstallationTarget;
  readonly acquisition: Acquisition;
  readonly replaceModified: boolean;
  readonly resources: readonly ResourcePlan[];
  readonly error: TargetError | null;
  readonly applicable: boolean;

  private constructor(parts: PreviewParts) {
    const wire: PreviewWire = callNative(() => parts.handle.snapshot());
    this.bundleId = wire.bundleId;
    this.bundle = parts.bundle;
    this.rendered = parts.rendered;
    this.target = parts.target;
    this.acquisition = wire.acquisition;
    this.replaceModified = wire.replaceModified;
    this.resources = Object.freeze(wire.resources.map(resourcePlanFromWire));
    this.error = targetErrorFromWire(wire.error);
    this.applicable = wire.applicable;
    attachHandle(this, parts.handle);
    Object.freeze(this);
  }

  static {
    createPreview = (parts) => new LifecyclePreview(parts);
  }
}

let createObservation: (
  wire: InstallationObservationWire,
  target: InstallationTarget,
) => InstallationObservation;

export class InstallationObservation {
  readonly bundleId: string;
  readonly target: InstallationTarget;
  readonly resources: readonly ResourceObservation[];
  readonly isCurrent: boolean;
  readonly pending: boolean;
  readonly version: string | null;
  readonly bundleDigest: string | null;
  readonly renderDigest: string | null;
  readonly error: TargetError | null;

  private constructor(wire: InstallationObservationWire, target: InstallationTarget) {
    this.bundleId = wire.bundleId;
    this.target = target;
    this.resources = Object.freeze(wire.resources.map(resourceObservationFromWire));
    this.isCurrent = wire.isCurrent;
    this.pending = wire.pending;
    this.version = wire.version;
    this.bundleDigest = wire.bundleDigest;
    this.renderDigest = wire.renderDigest;
    this.error = targetErrorFromWire(wire.error);
    attachHandle(this, wire.handle);
    Object.freeze(this);
  }

  matches(bundle: Bundle, rendered: RenderedBundle): boolean {
    return (handleOf(this) as InstallationObservationWire['handle']).matches(
      bundleHandle(bundle),
      renderedHandle(rendered),
    );
  }

  static {
    createObservation = (wire, target) => new InstallationObservation(wire, target);
  }
}

export const observationFromWire = (
  wire: InstallationObservationWire,
  target: InstallationTarget,
): InstallationObservation => createObservation(wire, target);

export const installationResultFromWire = (
  wire: InstallationResultWire,
  target: InstallationTarget,
): InstallationResult =>
  Object.freeze({
    status: wire.status,
    observation: createObservation(wire.observation, target),
    resources: Object.freeze(wire.resources.map(resourceResultFromWire)),
    notices: Object.freeze(wire.notices.map(noticeFromWire)),
    error: targetErrorFromWire(wire.error),
    succeeded: wire.status === 'applied' || wire.status === 'unchanged',
  });

export const previewFromHandle = (
  handle: NativeLifecyclePreview,
  bundle: Bundle | null,
  rendered: RenderedBundle | null,
  target: InstallationTarget,
): LifecyclePreview =>
  createPreview({
    handle,
    bundle,
    rendered: rendered ?? adoptRenderedBundle(handle.rendered()),
    target,
  });

export const previewHandle = (preview: LifecyclePreview): NativeLifecyclePreview =>
  requireHandle(
    preview,
    LifecyclePreview,
    native.NativeLifecyclePreview,
    'preview must be a LifecyclePreview',
  );

const lifecycleOptions = (
  options:
    | (LockOptions & { readonly acquisition?: Acquisition; readonly replaceModified?: boolean })
    | undefined,
  locks: boolean,
): JsLifecycleOptions => ({
  acquisition: options?.acquisition ?? 'conflict',
  replaceModified: options?.replaceModified ?? false,
  lockTimeoutMs: locks ? lockTimeout(options) : 0,
});

export function preview(
  bundle: Bundle,
  rendered: RenderedBundle,
  target: InstallationTarget,
  options?: PreviewOptions,
): Promise<LifecyclePreview> {
  return runReadOnly(options?.signal, async () => {
    const handle = await awaitNative(
      native.lifecyclePreview(
        bundleHandle(bundle),
        renderedHandle(rendered),
        installationHandle(target),
        lifecycleOptions(options, false),
      ),
    );
    return previewFromHandle(handle, bundle, rendered, target);
  });
}

export function previewRemoval(
  id: string,
  target: InstallationTarget,
  options?: CallOptions & { readonly replaceModified?: boolean },
): Promise<LifecyclePreview> {
  return runReadOnly(options?.signal, async () => {
    const handle = await awaitNative(
      native.lifecyclePreviewRemoval(
        requireBundleId(id),
        installationHandle(target),
        lifecycleOptions(options, false),
      ),
    );
    return previewFromHandle(handle, null, null, target);
  });
}

const mutate = (
  signal: AbortSignal | undefined,
  targetOf: () => InstallationTarget,
  start: (token: NativeCancelToken | null) => Promise<NativeResult<InstallationResultWire>>,
): Promise<InstallationResult> =>
  runMutation(signal, async (token) =>
    installationResultFromWire(await awaitNative(start(token)), targetOf()),
  );

export function applyPreview(
  proposal: LifecyclePreview,
  options?: LockOptions,
): Promise<InstallationResult> {
  return mutate(
    options?.signal,
    () => proposal.target,
    (token) =>
      native.lifecycleApplyPreview(previewHandle(proposal), lifecycleOptions(options, true), token),
  );
}

export function sync(
  bundle: Bundle,
  rendered: RenderedBundle,
  target: InstallationTarget,
  options?: SyncOptions,
): Promise<InstallationResult> {
  return mutate(
    options?.signal,
    () => target,
    (token) =>
      native.lifecycleSync(
        bundleHandle(bundle),
        renderedHandle(rendered),
        installationHandle(target),
        lifecycleOptions(options, true),
        token,
      ),
  );
}

export function recoverInstallation(
  id: string,
  target: InstallationTarget,
  options?: LockOptions,
): Promise<InstallationResult> {
  return mutate(
    options?.signal,
    () => target,
    (token) =>
      native.lifecycleRecoverInstallation(
        requireBundleId(id),
        installationHandle(target),
        lifecycleOptions(options, true),
        token,
      ),
  );
}

export function inspectInstallation(
  id: string,
  target: InstallationTarget,
  options?: CallOptions,
): Promise<InstallationObservation> {
  return runReadOnly(options?.signal, async () =>
    createObservation(
      await awaitNative(
        native.lifecycleInspectInstallation(requireBundleId(id), installationHandle(target)),
      ),
      target,
    ),
  );
}

export function remove(
  id: string,
  target: InstallationTarget,
  options?: RemoveOptions,
): Promise<InstallationResult> {
  return mutate(
    options?.signal,
    () => target,
    (token) =>
      native.lifecycleRemove(
        requireBundleId(id),
        installationHandle(target),
        lifecycleOptions(options, true),
        token,
      ),
  );
}
