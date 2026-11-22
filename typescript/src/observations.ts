import type { ModificationKind, ObservationState, OperationStatus } from './enums.js';
import type { TargetError } from './errors.js';
import type {
  InstallationWire,
  InventoryEntryWire,
  ObservationWire,
  SkillInspectionWire,
  SkillResultWire,
} from './internal/models.js';
import { frozenCopy, targetErrorFromWire } from './internal/results.js';
import type { Target } from './targets.js';

export interface InventoryEntry {
  readonly path: string;
  readonly directory: boolean;
  readonly size: bigint | null;
  readonly sha256: string | null;
  readonly executable: boolean;
}

export interface Installation {
  readonly bundleId: string;
  readonly version: string;
  readonly contentDigest: string;
  readonly transactionId: string;
  readonly entries: readonly InventoryEntry[];
}

export interface Modification {
  readonly bundleId: string;
  readonly path: string;
  readonly kind: ModificationKind;
}

export interface Conflict {
  readonly path: string;
  readonly owner: string | null;
}

export interface Observation {
  readonly state: ObservationState;
  readonly installed: Installation | null;
  readonly installations: readonly Installation[];
  readonly versionMatches: boolean | null;
  readonly recordedContentMatches: boolean | null;
  readonly contentMatches: boolean | null;
  readonly modifications: readonly Modification[];
  readonly conflicts: readonly Conflict[];
  readonly error: TargetError | null;
  readonly recoveryPaths: readonly string[];
  readonly isCurrent: boolean;
}

export interface TargetInspection {
  readonly target: Target;
  readonly root: string;
  readonly aliasOf: number | null;
  readonly observation: Observation;
}

export interface TargetResult {
  readonly target: Target;
  readonly root: string;
  readonly aliasOf: number | null;
  readonly status: OperationStatus;
  readonly observation: Observation;
  readonly error: TargetError | null;
  readonly recoveryPaths: readonly string[];
  readonly succeeded: boolean;
}

const entryFromWire = (wire: InventoryEntryWire): InventoryEntry =>
  Object.freeze({
    path: wire.path,
    directory: wire.directory,
    size: wire.size,
    sha256: wire.sha256,
    executable: wire.executable,
  });

const installationFromWire = (wire: InstallationWire): Installation =>
  Object.freeze({
    bundleId: wire.bundleId,
    version: wire.version,
    contentDigest: wire.contentDigest,
    transactionId: wire.transactionId,
    entries: Object.freeze(wire.entries.map(entryFromWire)),
  });

const observationFromWire = (wire: ObservationWire): Observation =>
  Object.freeze({
    state: wire.state,
    installed: wire.installed === null ? null : installationFromWire(wire.installed),
    installations: Object.freeze(wire.installations.map(installationFromWire)),
    versionMatches: wire.versionMatches,
    recordedContentMatches: wire.recordedContentMatches,
    contentMatches: wire.contentMatches,
    modifications: Object.freeze(
      wire.modifications.map((item) =>
        Object.freeze({ bundleId: item.bundleId, path: item.path, kind: item.kind }),
      ),
    ),
    conflicts: Object.freeze(
      wire.conflicts.map((item) => Object.freeze({ path: item.path, owner: item.owner })),
    ),
    error: targetErrorFromWire(wire.error),
    recoveryPaths: frozenCopy(wire.recoveryPaths),
    isCurrent: wire.isCurrent,
  });

const targetAt = (targets: readonly Target[], index: number): Target => {
  const target = targets[index];
  if (target === undefined) {
    throw new RangeError('native result has more entries than targets');
  }
  return target;
};

export const inspectionsFromWire = (
  wire: readonly SkillInspectionWire[],
  targets: readonly Target[],
): readonly TargetInspection[] =>
  Object.freeze(
    wire.map((item, index) =>
      Object.freeze({
        target: targetAt(targets, index),
        root: item.root,
        aliasOf: item.aliasOf,
        observation: observationFromWire(item.observation),
      }),
    ),
  );

export const targetResultsFromWire = (
  wire: readonly SkillResultWire[],
  targets: readonly Target[],
): readonly TargetResult[] =>
  Object.freeze(
    wire.map((item, index) =>
      Object.freeze({
        target: targetAt(targets, index),
        root: item.root,
        aliasOf: item.aliasOf,
        status: item.status,
        observation: observationFromWire(item.observation),
        error: targetErrorFromWire(item.error),
        recoveryPaths: frozenCopy(item.recoveryPaths),
        succeeded: item.status === 'applied' || item.status === 'unchanged',
      }),
    ),
  );
