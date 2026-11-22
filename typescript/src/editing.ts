import { Claim, claimFromWire, claimWire } from './claims.js';
import {
  Selector,
  contentFromWire,
  contentWire,
  selectorFromWire,
  selectorWire,
  type Content,
  Member,
} from './content.js';
import type { Acquisition, Disposition, DocumentFormat, EditStatus } from './enums.js';
import { ValueError } from './errors.js';
import { callNative } from './internal/call.js';
import { fromNative, toNative } from './internal/convert.js';
import { attachHandle, handleOf } from './internal/handles.js';
import {
  buildModel,
  modelHandle,
  type ArrayPositionWire,
  type CreatedContainerWire,
  type DocumentEditWire,
  type EditObservationWire,
  type MemberWire,
  type OwnedSelectionWire,
  type ProvenanceWire,
  type RequestWire,
  type SnapshotValueWire,
  type SnapshotWire,
} from './internal/models.js';
import { requireText } from './internal/text.js';
import type { NativeModel } from './native/binding.js';
import { native } from './native/loader.js';
import type { SemanticValue } from './values.js';

export type SnapshotValue =
  | { readonly kind: 'bytes'; readonly bytes: Uint8Array }
  | { readonly kind: 'typed'; readonly value: SemanticValue };
export type SharedContent = Content;

const copy = (bytes: Uint8Array): Uint8Array => {
  if (!(bytes instanceof Uint8Array)) {
    throw new ValueError('bytes must be a Uint8Array');
  }
  return new Uint8Array(bytes);
};
const copyOrNull = (bytes: Uint8Array | null | undefined): Uint8Array | null =>
  bytes === null || bytes === undefined ? null : copy(bytes);

const memberWire = (member: Member): MemberWire => ({
  kind: 'member',
  identity: toNative(member.identity),
  key: member.key,
});
const memberFromWire = (wire: MemberWire): Member =>
  new Member(fromNative(wire.identity), wire.key);
const optionalMember = (wire: MemberWire | null): Member | null =>
  wire === null ? null : memberFromWire(wire);

export class ArrayPosition {
  readonly previous: Member | null;
  readonly following: Member | null;
  readonly #prefix: Uint8Array;

  constructor(previous: Member | null, following: Member | null, prefix: Uint8Array) {
    const { handle, snapshot } = buildModel<ArrayPositionWire>({
      kind: 'array-position',
      previous: previous === null ? null : memberWire(previous),
      following: following === null ? null : memberWire(following),
      prefix: copy(prefix),
    });
    this.previous = optionalMember(snapshot.previous);
    this.following = optionalMember(snapshot.following);
    this.#prefix = copy(snapshot.prefix);
    attachHandle(this, handle);
    Object.freeze(this);
  }

  get prefix(): Uint8Array {
    return copy(this.#prefix);
  }
}

const positionWire = (position: ArrayPosition): ArrayPositionWire => ({
  kind: 'array-position',
  previous: position.previous === null ? null : memberWire(position.previous),
  following: position.following === null ? null : memberWire(position.following),
  prefix: position.prefix,
});
const positionFromWire = (wire: ArrayPositionWire): ArrayPosition =>
  new ArrayPosition(optionalMember(wire.previous), optionalMember(wire.following), wire.prefix);

const snapshotValueWire = (value: SnapshotValue): SnapshotValueWire => {
  switch (value.kind) {
    case 'bytes':
      return { kind: 'bytes', bytes: copy(value.bytes) };
    case 'typed':
      return { kind: 'typed', value: toNative(value.value) };
    default:
      throw new ValueError('snapshot value has an unknown kind');
  }
};
const snapshotValueFromWire = (wire: SnapshotValueWire): SnapshotValue =>
  wire.kind === 'bytes'
    ? Object.freeze({ kind: 'bytes', bytes: copy(wire.bytes) } as const)
    : Object.freeze({ kind: 'typed', value: fromNative(wire.value) } as const);

export interface SelectionSnapshotInit {
  readonly value: SnapshotValue;
  readonly syntax: Uint8Array;
  readonly ownedSyntax: Uint8Array;
  readonly position?: ArrayPosition | null;
  readonly layout: Uint8Array;
}

export class SelectionSnapshot {
  readonly position: ArrayPosition | null;
  readonly #value: SnapshotValueWire;
  readonly #syntax: Uint8Array;
  readonly #ownedSyntax: Uint8Array;
  readonly #layout: Uint8Array;

  constructor(init: SelectionSnapshotInit) {
    const { handle, snapshot } = buildModel<SnapshotWire>({
      kind: 'snapshot',
      value: snapshotValueWire(init.value),
      syntax: copy(init.syntax),
      ownedSyntax: copy(init.ownedSyntax),
      position:
        init.position === undefined || init.position === null ? null : positionWire(init.position),
      layout: copy(init.layout),
    });
    this.#value = snapshot.value;
    this.#syntax = copy(snapshot.syntax);
    this.#ownedSyntax = copy(snapshot.ownedSyntax);
    this.#layout = copy(snapshot.layout);
    this.position = snapshot.position === null ? null : positionFromWire(snapshot.position);
    attachHandle(this, handle);
    Object.freeze(this);
  }

  get value(): SnapshotValue {
    return snapshotValueFromWire(this.#value);
  }

  get syntax(): Uint8Array {
    return copy(this.#syntax);
  }

  get ownedSyntax(): Uint8Array {
    return copy(this.#ownedSyntax);
  }

  get layout(): Uint8Array {
    return copy(this.#layout);
  }
}

const snapshotWire = (snapshot: SelectionSnapshot): SnapshotWire => ({
  kind: 'snapshot',
  value: snapshotValueWire(snapshot.value),
  syntax: snapshot.syntax,
  ownedSyntax: snapshot.ownedSyntax,
  position: snapshot.position === null ? null : positionWire(snapshot.position),
  layout: snapshot.layout,
});
const snapshotFromWire = (wire: SnapshotWire): SelectionSnapshot =>
  new SelectionSnapshot({
    value: snapshotValueFromWire(wire.value),
    syntax: wire.syntax,
    ownedSyntax: wire.ownedSyntax,
    position: wire.position === null ? null : positionFromWire(wire.position),
    layout: wire.layout,
  });

export interface OwnedSelectionInit {
  readonly claim: Claim;
  readonly format?: DocumentFormat | null;
  readonly installed: SelectionSnapshot;
  readonly disposition: Disposition;
  readonly baseline?: SelectionSnapshot | null;
  readonly separator?: Uint8Array | null;
}

export class OwnedSelection {
  readonly claim: Claim;
  readonly format: DocumentFormat | null;
  readonly installed: SelectionSnapshot;
  readonly disposition: Disposition;
  readonly baseline: SelectionSnapshot | null;
  readonly #separator: Uint8Array | null;

  constructor(init: OwnedSelectionInit) {
    const { handle, snapshot } = buildModel<OwnedSelectionWire>({
      kind: 'owned-selection',
      claim: claimWire(init.claim),
      format: init.format ?? null,
      installed: snapshotWire(init.installed),
      disposition: init.disposition,
      baseline:
        init.baseline === undefined || init.baseline === null ? null : snapshotWire(init.baseline),
      separator: copyOrNull(init.separator),
    });
    this.claim = claimFromWire(snapshot.claim);
    this.format = snapshot.format;
    this.installed = snapshotFromWire(snapshot.installed);
    this.disposition = snapshot.disposition;
    this.baseline = snapshot.baseline === null ? null : snapshotFromWire(snapshot.baseline);
    this.#separator = copyOrNull(snapshot.separator);
    attachHandle(this, handle);
    Object.freeze(this);
  }

  get separator(): Uint8Array | null {
    return copyOrNull(this.#separator);
  }
}

const selectionWire = (selection: OwnedSelection): OwnedSelectionWire => ({
  kind: 'owned-selection',
  claim: claimWire(selection.claim),
  format: selection.format,
  installed: snapshotWire(selection.installed),
  disposition: selection.disposition,
  baseline: selection.baseline === null ? null : snapshotWire(selection.baseline),
  separator: selection.separator,
});
const selectionFromWire = (wire: OwnedSelectionWire): OwnedSelection =>
  new OwnedSelection({
    claim: claimFromWire(wire.claim),
    format: wire.format,
    installed: snapshotFromWire(wire.installed),
    disposition: wire.disposition,
    baseline: wire.baseline === null ? null : snapshotFromWire(wire.baseline),
    separator: wire.separator,
  });

export class CreatedContainer {
  readonly selector: Selector;
  readonly #emptySyntax: Uint8Array;

  constructor(selector: Selector, emptySyntax: Uint8Array) {
    const { handle, snapshot } = buildModel<CreatedContainerWire>({
      kind: 'created-container',
      selector: selectorWire(selector),
      emptySyntax: copy(emptySyntax),
    });
    this.selector = selectorFromWire(snapshot.selector);
    this.#emptySyntax = copy(snapshot.emptySyntax);
    attachHandle(this, handle);
    Object.freeze(this);
  }

  get emptySyntax(): Uint8Array {
    return copy(this.#emptySyntax);
  }
}

const containerWire = (container: CreatedContainer): CreatedContainerWire => ({
  kind: 'created-container',
  selector: selectorWire(container.selector),
  emptySyntax: container.emptySyntax,
});
const containerFromWire = (wire: CreatedContainerWire): CreatedContainer =>
  new CreatedContainer(selectorFromWire(wire.selector), wire.emptySyntax);

export interface DocumentProvenanceInit {
  readonly emptyDocument?: Uint8Array | null;
  readonly containers?: readonly CreatedContainer[];
  readonly trailingSeparators?: readonly string[];
  readonly schemaFields?: readonly OwnedSelection[];
  readonly schemaEmptyDocument?: Uint8Array | null;
}

export class DocumentProvenance {
  readonly containers: readonly CreatedContainer[];
  readonly trailingSeparators: readonly string[];
  readonly schemaFields: readonly OwnedSelection[];
  readonly #emptyDocument: Uint8Array | null;
  readonly #schemaEmptyDocument: Uint8Array | null;

  constructor(init: DocumentProvenanceInit = {}) {
    const { handle, snapshot } = buildModel<ProvenanceWire>({
      kind: 'provenance',
      emptyDocument: copyOrNull(init.emptyDocument),
      containers: (init.containers ?? []).map(containerWire),
      trailingSeparators: (init.trailingSeparators ?? []).map((digest) =>
        requireText(digest, 'trailing separator'),
      ),
      schemaFields: (init.schemaFields ?? []).map(selectionWire),
      schemaEmptyDocument: copyOrNull(init.schemaEmptyDocument),
    });
    this.#emptyDocument = copyOrNull(snapshot.emptyDocument);
    this.containers = Object.freeze(snapshot.containers.map(containerFromWire));
    this.trailingSeparators = Object.freeze([...snapshot.trailingSeparators]);
    this.schemaFields = Object.freeze(snapshot.schemaFields.map(selectionFromWire));
    this.#schemaEmptyDocument = copyOrNull(snapshot.schemaEmptyDocument);
    attachHandle(this, handle);
    Object.freeze(this);
  }

  get emptyDocument(): Uint8Array | null {
    return copyOrNull(this.#emptyDocument);
  }

  get schemaEmptyDocument(): Uint8Array | null {
    return copyOrNull(this.#schemaEmptyDocument);
  }
}

const provenanceFromWire = (wire: ProvenanceWire): DocumentProvenance =>
  new DocumentProvenance({
    emptyDocument: wire.emptyDocument,
    containers: wire.containers.map(containerFromWire),
    trailingSeparators: wire.trailingSeparators,
    schemaFields: wire.schemaFields.map(selectionFromWire),
    schemaEmptyDocument: wire.schemaEmptyDocument,
  });

export class DocumentRequest {
  readonly claim: Claim;
  readonly content: Content | null;
  readonly acquisition: Acquisition;

  constructor(claim: Claim, content: Content | null, acquisition: Acquisition = 'conflict') {
    const { handle, snapshot } = buildModel<RequestWire>({
      kind: 'request',
      claim: claimWire(claim),
      content: content === null ? null : contentWire(content),
      acquisition,
    });
    this.claim = claim;
    this.content = snapshot.content === null ? null : contentFromWire(snapshot.content);
    this.acquisition = snapshot.acquisition;
    attachHandle(this, handle);
    Object.freeze(this);
  }

  static set(content: Content, acquisition: Acquisition = 'conflict'): DocumentRequest {
    return new DocumentRequest(contentClaim(content), content, acquisition);
  }

  static retire(claim: Claim): DocumentRequest {
    return new DocumentRequest(claim, null);
  }
}

export interface EditObservation {
  readonly claim: Claim;
  readonly status: EditStatus;
  readonly current: SelectionSnapshot | null;
  readonly message: string;
}

export interface DocumentEdit {
  readonly before: Uint8Array | null;
  readonly after: Uint8Array | null;
  readonly ownership: readonly OwnedSelection[];
  readonly provenance: DocumentProvenance;
  readonly observations: readonly EditObservation[];
  readonly applicable: boolean;
}

const observationFromWire = (wire: EditObservationWire): EditObservation =>
  Object.freeze({
    claim: claimFromWire(wire.claim),
    status: wire.status,
    current: wire.current === null ? null : snapshotFromWire(wire.current),
    message: wire.message,
  });

const editFromWire = (wire: DocumentEditWire): DocumentEdit =>
  Object.freeze({
    before: wire.before,
    after: wire.after,
    ownership: Object.freeze(wire.ownership.map(selectionFromWire)),
    provenance: provenanceFromWire(wire.provenance),
    observations: Object.freeze(wire.observations.map(observationFromWire)),
    applicable: wire.applicable,
  });

const handlesOf = <T extends object>(
  items: readonly T[],
  type: abstract new (...args: never[]) => T,
  label: string,
): NativeModel[] =>
  items.map((item) => {
    if (!(item instanceof type) || handleOf(item) === undefined) {
      throw new ValueError(`${label} must be a ${type.name}`);
    }
    return modelHandle(item);
  });

export function editDocument(
  source: Uint8Array | null,
  requests: readonly DocumentRequest[],
  ownership: readonly OwnedSelection[] = [],
  provenance: DocumentProvenance = new DocumentProvenance(),
): DocumentEdit {
  const data = copyOrNull(source);
  const [handle] = handlesOf([provenance], DocumentProvenance, 'provenance');
  return editFromWire(
    callNative(() =>
      native.editingEditDocument(
        data,
        handlesOf(requests, DocumentRequest, 'request'),
        handlesOf(ownership, OwnedSelection, 'ownership'),
        handle as NativeModel,
      ),
    ),
  );
}

export function contentClaim(content: Content): Claim {
  if (handleOf(content) === undefined) {
    throw new ValueError('content must be a section or structured content');
  }
  return claimFromWire(callNative(() => native.editingContentClaim(modelHandle(content))));
}
