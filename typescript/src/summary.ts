import type { Agent, ChangeAction, Family, ResourceKind, Surface, TargetScope } from './enums.js';
import type { TargetError } from './errors.js';
import { callNative } from './internal/call.js';
import type { PlannedChangeWire } from './internal/models.js';
import { frozenPaths, targetErrorFromWire } from './internal/results.js';
import { previewHandle, type LifecyclePreview } from './lifecycle.js';
import { native } from './native/loader.js';
import { noticeFromWire, type Notice } from './rendered.js';

export interface PlannedChange {
  readonly artifactIds: readonly string[];
  readonly families: readonly Family[];
  readonly kind: ResourceKind;
  readonly destination: string;
  readonly action: ChangeAction;
  readonly error: TargetError | null;
  readonly recoveryPaths: readonly string[];
}

export interface PlanSummary {
  readonly agent: Agent | null;
  readonly scope: TargetScope | null;
  readonly surface: Surface | null;
  readonly applicable: boolean;
  readonly changes: readonly PlannedChange[];
  readonly notices: readonly Notice[];
  readonly error: TargetError | null;
}

const changeFromWire = (wire: PlannedChangeWire): PlannedChange =>
  Object.freeze({
    artifactIds: Object.freeze([...wire.artifactIds]),
    families: Object.freeze([...wire.families]),
    kind: wire.kind,
    destination: wire.destination,
    action: wire.action,
    error: targetErrorFromWire(wire.error),
    recoveryPaths: frozenPaths(wire.recoveryPaths),
  });

export function summarize(preview: LifecyclePreview): PlanSummary {
  const wire = callNative(() => native.lifecycleSummarize(previewHandle(preview)));
  return Object.freeze({
    agent: wire.agent,
    scope: wire.scope,
    surface: wire.surface,
    applicable: wire.applicable,
    changes: Object.freeze(wire.changes.map(changeFromWire)),
    notices: Object.freeze(wire.notices.map(noticeFromWire)),
    error: targetErrorFromWire(wire.error),
  });
}
