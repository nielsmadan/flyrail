import type {
  Acquisition,
  Agent,
  ChangeAction,
  DependencyMode,
  Disposition,
  DocumentFormat,
  EditStatus,
  ErrorCode,
  Family,
  FileMode,
  HookEvent,
  HookOutcome,
  ModificationKind,
  NoticeKind,
  ObservationState,
  OperationStatus,
  Ownership,
  Platform,
  Position,
  ResourceKind,
  Surface,
  TargetScope,
} from '../enums.js';
import type {
  JsBundleEntry,
  NativeInstallationObservation,
  NativeModel,
} from '../native/binding.js';
import { native } from '../native/loader.js';
import { callNative } from './call.js';
import type { NativeValue } from './convert.js';
import { requireHandle, type Brand } from './handles.js';

export interface AudienceWire {
  readonly kind: 'audience';
  readonly agent: Agent;
  readonly scope: TargetScope;
  readonly surface: Surface;
  readonly platform: Platform | null;
}
export interface EnvRefWire {
  readonly kind: 'env-ref';
  readonly name: string;
}
export interface AssetRefWire {
  readonly kind: 'asset-ref';
  readonly assetId: string;
  readonly path: string | null;
}
export interface TextWire {
  readonly kind: 'text';
  readonly text: string;
}
export type ArgumentWire = TextWire | AssetRefWire;
export type EnvironmentValueWire = TextWire | EnvRefWire;
export interface CommandWire {
  readonly kind: 'command';
  readonly argv: readonly ArgumentWire[];
  readonly env: readonly { readonly name: string; readonly value: EnvironmentValueWire }[];
  readonly cwd: ArgumentWire | null;
}
export interface HttpWire {
  readonly kind: 'http';
  readonly url: string;
  readonly bearerToken: EnvRefWire | null;
}
export interface FileWire {
  readonly kind: 'file';
  readonly data: Uint8Array;
  readonly mode: FileMode;
}
export interface TreeWire {
  readonly kind: 'tree';
  readonly entries: readonly JsBundleEntry[];
}
export interface BoundariesWire {
  readonly kind: 'boundaries';
  readonly start: string;
  readonly end: string;
}
export interface BoundariesMarkerWire {
  readonly kind: 'boundaries-marker';
  readonly marker: string;
}
export interface SectionWire {
  readonly kind: 'section';
  readonly marker: string;
  readonly text: string;
  readonly boundaries: BoundariesWire | null;
}
export interface KeyWire {
  readonly kind: 'key';
  readonly name: string;
}
export interface MemberWire {
  readonly kind: 'member';
  readonly identity: NativeValue;
  readonly key: readonly string[];
}
export interface SelectorWire {
  readonly kind: 'selector';
  readonly parts: readonly (KeyWire | MemberWire)[];
}
export interface PlacementWire {
  readonly kind: 'placement';
  readonly position: Position;
  readonly anchor: MemberWire | null;
}
export interface StructuredWire {
  readonly kind: 'structured';
  readonly format: DocumentFormat;
  readonly selector: SelectorWire;
  readonly value: NativeValue;
  readonly placement: PlacementWire | null;
}
export interface SchemaWire {
  readonly kind: 'schema';
  readonly format: DocumentFormat;
  readonly key: string;
  readonly value: NativeValue;
}
export type ContentWire = FileWire | TreeWire | SectionWire | StructuredWire;
export interface SkillWire {
  readonly kind: 'skill';
  readonly id: string;
  readonly name: string;
  readonly content: TreeWire;
}
export interface InstructionWire {
  readonly kind: 'instruction';
  readonly id: string;
  readonly text: string;
}
export interface McpWire {
  readonly kind: 'mcp';
  readonly id: string;
  readonly name: string;
  readonly transport: CommandWire | HttpWire;
}
export interface HookWire {
  readonly kind: 'hook';
  readonly id: string;
  readonly event: HookEvent;
  readonly command: CommandWire;
  readonly timeoutMs: number;
  readonly outcomes: readonly HookOutcome[];
  readonly tools: readonly string[];
}
export interface NativeArtifactWire {
  readonly kind: 'native';
  readonly id: string;
  readonly family: Family;
  readonly audience: AudienceWire;
  readonly destination: string;
  readonly content: ContentWire;
}
export type ArtifactWire = SkillWire | InstructionWire | McpWire | HookWire | NativeArtifactWire;
export interface AssetWire {
  readonly kind: 'asset';
  readonly id: string;
  readonly family: Family;
  readonly content: TreeWire;
}
export interface DependencyWire {
  readonly kind: 'dependency';
  readonly dependent: string;
  readonly required: string;
  readonly mode: DependencyMode;
}
export type ClaimSelectorWire =
  | { readonly kind: 'whole' }
  | { readonly kind: 'subtree'; readonly path: string }
  | BoundariesWire
  | SelectorWire;
export interface ClaimWire {
  readonly kind: 'claim';
  readonly ownership: Ownership;
  readonly selector: ClaimSelectorWire;
}

export interface ArrayPositionWire {
  readonly kind: 'array-position';
  readonly previous: MemberWire | null;
  readonly following: MemberWire | null;
  readonly prefix: Uint8Array;
}
export type SnapshotValueWire =
  | { readonly kind: 'bytes'; readonly bytes: Uint8Array }
  | { readonly kind: 'typed'; readonly value: NativeValue };
export interface SnapshotWire {
  readonly kind: 'snapshot';
  readonly value: SnapshotValueWire;
  readonly syntax: Uint8Array;
  readonly ownedSyntax: Uint8Array;
  readonly position: ArrayPositionWire | null;
  readonly layout: Uint8Array;
}
export interface OwnedSelectionWire {
  readonly kind: 'owned-selection';
  readonly claim: ClaimWire;
  readonly format: DocumentFormat | null;
  readonly installed: SnapshotWire;
  readonly disposition: Disposition;
  readonly baseline: SnapshotWire | null;
  readonly separator: Uint8Array | null;
}
export interface CreatedContainerWire {
  readonly kind: 'created-container';
  readonly selector: SelectorWire;
  readonly emptySyntax: Uint8Array;
}
export interface ProvenanceWire {
  readonly kind: 'provenance';
  readonly emptyDocument: Uint8Array | null;
  readonly containers: readonly CreatedContainerWire[];
  readonly trailingSeparators: readonly string[];
  readonly schemaFields: readonly OwnedSelectionWire[];
  readonly schemaEmptyDocument: Uint8Array | null;
}
export interface RequestWire {
  readonly kind: 'request';
  readonly claim: ClaimWire;
  readonly content: ContentWire | null;
  readonly acquisition: Acquisition;
}
export interface EditObservationWire {
  readonly kind: 'edit-observation';
  readonly claim: ClaimWire;
  readonly status: EditStatus;
  readonly current: SnapshotWire | null;
  readonly message: string;
}
export interface DocumentEditWire {
  readonly before: Uint8Array | null;
  readonly after: Uint8Array | null;
  readonly ownership: readonly OwnedSelectionWire[];
  readonly provenance: ProvenanceWire;
  readonly observations: readonly EditObservationWire[];
  readonly applicable: boolean;
}

export interface LoadoutLocationWire {
  readonly kind: 'loadout-location';
  readonly root: string;
  readonly profile: string;
  readonly sourceName: string;
  readonly sourcePath: string;
}

export type ModelWire =
  | AudienceWire
  | EnvRefWire
  | AssetRefWire
  | CommandWire
  | HttpWire
  | ContentWire
  | BoundariesWire
  | BoundariesMarkerWire
  | KeyWire
  | MemberWire
  | SelectorWire
  | PlacementWire
  | SchemaWire
  | ArtifactWire
  | AssetWire
  | DependencyWire
  | ClaimWire
  | ArrayPositionWire
  | SnapshotWire
  | OwnedSelectionWire
  | CreatedContainerWire
  | ProvenanceWire
  | RequestWire
  | LoadoutLocationWire;

export interface Built<W extends ModelWire> {
  readonly handle: NativeModel;
  readonly snapshot: W;
}

export function buildModel<W extends ModelWire>(wire: ModelWire): Built<W> {
  const handle = callNative(() => native.modelsBuild(wire));
  const snapshot = callNative(() => handle.snapshot()) as W;
  return { handle, snapshot };
}

export const modelHandle = (owner: unknown, type: Brand, message: string): NativeModel =>
  requireHandle(owner, type, native.NativeModel, message);

export interface NoticeWire {
  readonly kind: 'notice';
  readonly artifactId: string;
  readonly noticeKind: NoticeKind;
  readonly message: string;
  readonly code: string | null;
}
export interface PairWire {
  readonly key: string;
  readonly value: string;
}
export interface RenderedArtifactWire {
  readonly kind: 'rendered-artifact';
  readonly id: string;
  readonly family: Family;
  readonly destination: string;
  readonly content: ContentWire;
  readonly subtree: string | null;
  readonly requireExisting: boolean;
  readonly schemaRequirements: readonly SchemaWire[];
}
export interface RenderedBundleInputWire {
  readonly artifacts: readonly RenderedArtifactWire[];
  readonly dependencies: readonly DependencyWire[];
  readonly notices: readonly NoticeWire[];
  readonly routingContext: readonly PairWire[];
}
export interface RenderedBundleWire extends RenderedBundleInputWire {
  readonly contentDigest: string;
  readonly supported: boolean;
}
export interface TranslationWire {
  readonly artifact: RenderedArtifactWire | null;
  readonly notices: readonly NoticeWire[];
  readonly supported: boolean;
}

export interface TargetErrorWire {
  readonly code: ErrorCode;
  readonly message: string;
  readonly path: string | null;
  readonly errno: number | null;
}
export interface ResourceRefWire {
  readonly destination: string;
  readonly kind: ResourceKind;
}
export interface ResourcePlanWire {
  readonly resource: ResourceRefWire;
  readonly transactionDirectory: string;
  readonly recoveryPaths: readonly string[];
  readonly requireExisting: boolean;
  readonly error: TargetErrorWire | null;
}
export interface PreviewWire {
  readonly bundleId: string;
  readonly acquisition: Acquisition;
  readonly replaceModified: boolean;
  readonly resources: readonly ResourcePlanWire[];
  readonly error: TargetErrorWire | null;
  readonly applicable: boolean;
}
export interface ClaimObservationWire {
  readonly claimId: string;
  readonly bundleId: string;
  readonly artifactId: string;
  readonly version: string;
  readonly bundleDigest: string;
  readonly renderDigest: string;
  readonly status: EditStatus;
}
export interface ResourceObservationWire {
  readonly destination: string;
  readonly claims: readonly ClaimObservationWire[];
  readonly revisionDigest: string | null;
  readonly error: TargetErrorWire | null;
  readonly recoveryPaths: readonly string[];
}
export interface InstallationObservationWire {
  readonly handle: NativeInstallationObservation;
  readonly bundleId: string;
  readonly resources: readonly ResourceObservationWire[];
  readonly isCurrent: boolean;
  readonly pending: boolean;
  readonly version: string | null;
  readonly bundleDigest: string | null;
  readonly renderDigest: string | null;
  readonly error: TargetErrorWire | null;
}
export interface ResourceResultWire {
  readonly resource: ResourceRefWire;
  readonly status: OperationStatus;
  readonly error: TargetErrorWire | null;
  readonly recoveryPaths: readonly string[];
}
export interface InstallationResultWire {
  readonly status: OperationStatus;
  readonly observation: InstallationObservationWire;
  readonly resources: readonly ResourceResultWire[];
  readonly notices: readonly NoticeWire[];
  readonly error: TargetErrorWire | null;
}
export interface PlannedChangeWire {
  readonly artifactIds: readonly string[];
  readonly families: readonly Family[];
  readonly kind: ResourceKind;
  readonly destination: string;
  readonly action: ChangeAction;
  readonly error: TargetErrorWire | null;
  readonly recoveryPaths: readonly string[];
}
export interface PlanSummaryWire {
  readonly agent: Agent | null;
  readonly scope: TargetScope | null;
  readonly surface: Surface | null;
  readonly applicable: boolean;
  readonly changes: readonly PlannedChangeWire[];
  readonly notices: readonly NoticeWire[];
  readonly error: TargetErrorWire | null;
}

export interface InventoryEntryWire {
  readonly path: string;
  readonly directory: boolean;
  readonly size: bigint | null;
  readonly sha256: string | null;
  readonly executable: boolean;
}
export interface InstallationWire {
  readonly bundleId: string;
  readonly version: string;
  readonly contentDigest: string;
  readonly transactionId: string;
  readonly entries: readonly InventoryEntryWire[];
}
export interface ObservationWire {
  readonly state: ObservationState;
  readonly installed: InstallationWire | null;
  readonly installations: readonly InstallationWire[];
  readonly versionMatches: boolean | null;
  readonly recordedContentMatches: boolean | null;
  readonly contentMatches: boolean | null;
  readonly modifications: readonly {
    readonly bundleId: string;
    readonly path: string;
    readonly kind: ModificationKind;
  }[];
  readonly conflicts: readonly { readonly path: string; readonly owner: string | null }[];
  readonly error: TargetErrorWire | null;
  readonly recoveryPaths: readonly string[];
  readonly isCurrent: boolean;
}
export interface SkillInspectionWire {
  readonly root: string;
  readonly aliasOf: number | null;
  readonly observation: ObservationWire;
}
export interface SkillResultWire extends SkillInspectionWire {
  readonly status: OperationStatus;
  readonly error: TargetErrorWire | null;
  readonly recoveryPaths: readonly string[];
}
export interface AgentPresenceWire {
  readonly agent: Agent;
  readonly executable: string | null;
  readonly application: string | null;
}
