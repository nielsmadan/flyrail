import type { BundleEntry } from './bundle.js';
import type { DocumentFormat, FileMode, Position } from './enums.js';
import { fromNative, toNative, type NativeValue } from './internal/convert.js';
import { ValueError } from './errors.js';
import { entryFromNative, entryToNative } from './internal/entries.js';
import { attachHandle } from './internal/handles.js';
import {
  buildModel,
  type BoundariesWire,
  type ContentWire,
  type FileWire,
  type KeyWire,
  type MemberWire,
  type PlacementWire,
  type SchemaWire,
  type SectionWire,
  type SelectorWire,
  type StructuredWire,
  type TreeWire,
} from './internal/models.js';
import { requireText } from './internal/text.js';
import type { SemanticScalar, SemanticValue } from './values.js';

export class FileContent {
  readonly kind = 'file';
  readonly mode: FileMode;
  readonly #data: Uint8Array;

  constructor(data: Uint8Array, mode: FileMode) {
    const { handle, snapshot } = buildModel<FileWire>({
      kind: 'file',
      data: new Uint8Array(data),
      mode,
    });
    this.#data = new Uint8Array(snapshot.data);
    this.mode = snapshot.mode;
    attachHandle(this, handle);
    Object.freeze(this);
  }

  get data(): Uint8Array {
    return new Uint8Array(this.#data);
  }
}

const copyEntry = (entry: BundleEntry): BundleEntry =>
  Object.freeze({ ...entry, data: entry.data === null ? null : new Uint8Array(entry.data) });

export class TreeContent {
  readonly kind = 'tree';
  readonly #entries: readonly BundleEntry[];

  constructor(entries: readonly BundleEntry[]) {
    const { handle, snapshot } = buildModel<TreeWire>({
      kind: 'tree',
      entries: entries.map(entryToNative),
    });
    this.#entries = Object.freeze(snapshot.entries.map(entryFromNative));
    attachHandle(this, handle);
    Object.freeze(this);
  }

  get entries(): readonly BundleEntry[] {
    return Object.freeze(this.#entries.map(copyEntry));
  }
}

export class Key {
  readonly kind = 'key';
  readonly name: string;

  constructor(name: string) {
    const { handle, snapshot } = buildModel<KeyWire>({
      kind: 'key',
      name: requireText(name, 'selector key'),
    });
    this.name = snapshot.name;
    attachHandle(this, handle);
    Object.freeze(this);
  }
}

export class Member {
  readonly kind = 'member';
  readonly key: readonly string[];
  readonly #identity: NativeValue;

  constructor(identity: SemanticValue, key: readonly string[]) {
    const { handle, snapshot } = buildModel<MemberWire>({
      kind: 'member',
      identity: toNative(identity),
      key: key.map((name) => requireText(name, 'member key')),
    });
    this.#identity = snapshot.identity;
    this.key = Object.freeze([...snapshot.key]);
    attachHandle(this, handle);
    Object.freeze(this);
  }

  get identity(): SemanticValue {
    return fromNative(this.#identity);
  }
}

export type SelectorPart = Key | Member;

const partWire = (part: SelectorPart): KeyWire | MemberWire => {
  switch (part.kind) {
    case 'key':
      return { kind: 'key', name: part.name };
    case 'member':
      return { kind: 'member', identity: toNative(part.identity), key: part.key };
    default:
      throw new ValueError('selector part must be a Key or a Member');
  }
};

const partFromWire = (wire: KeyWire | MemberWire): SelectorPart =>
  wire.kind === 'key' ? new Key(wire.name) : new Member(fromNative(wire.identity), wire.key);

export class Selector {
  readonly kind = 'structured';
  readonly parts: readonly SelectorPart[];

  constructor(parts: readonly SelectorPart[]) {
    const { handle } = buildModel<SelectorWire>(selectorWire({ parts }));
    this.parts = Object.freeze([...parts]);
    attachHandle(this, handle);
    Object.freeze(this);
  }
}

export const selectorWire = (selector: {
  readonly parts: readonly SelectorPart[];
}): SelectorWire => ({
  kind: 'selector',
  parts: selector.parts.map(partWire),
});

export const selectorFromWire = (wire: SelectorWire): Selector =>
  new Selector(wire.parts.map(partFromWire));

export class Placement {
  readonly position: Position;
  readonly anchor: Member | null;

  constructor(position: Position = 'last', anchor: Member | null = null) {
    const { handle, snapshot } = buildModel<PlacementWire>({
      kind: 'placement',
      position,
      anchor: anchor === null ? null : partWireMember(anchor),
    });
    this.position = snapshot.position;
    this.anchor = anchor;
    attachHandle(this, handle);
    Object.freeze(this);
  }
}

const partWireMember = (member: Member): MemberWire => ({
  kind: 'member',
  identity: toNative(member.identity),
  key: member.key,
});

const placementWire = (placement: Placement): PlacementWire => ({
  kind: 'placement',
  position: placement.position,
  anchor: placement.anchor === null ? null : partWireMember(placement.anchor),
});

const placementFromWire = (wire: PlacementWire): Placement =>
  new Placement(
    wire.position,
    wire.anchor === null ? null : new Member(fromNative(wire.anchor.identity), wire.anchor.key),
  );

export class SectionBoundaries {
  readonly kind = 'section';
  readonly start: string;
  readonly end: string;

  constructor(start: string, end: string) {
    const { handle, snapshot } = buildModel<BoundariesWire>({
      kind: 'boundaries',
      start: requireText(start, 'section start'),
      end: requireText(end, 'section end'),
    });
    this.start = snapshot.start;
    this.end = snapshot.end;
    attachHandle(this, handle);
    Object.freeze(this);
  }

  static fromMarker(marker: string): SectionBoundaries {
    const { snapshot } = buildModel<BoundariesWire>({
      kind: 'boundaries-marker',
      marker: requireText(marker, 'section marker'),
    });
    return new SectionBoundaries(snapshot.start, snapshot.end);
  }
}

export const boundariesWire = (boundaries: SectionBoundaries): BoundariesWire => ({
  kind: 'boundaries',
  start: boundaries.start,
  end: boundaries.end,
});

export class SectionContent {
  readonly kind = 'section';
  readonly marker: string;
  readonly text: string;
  readonly boundaries: SectionBoundaries | null;
  readonly selector: SectionBoundaries;

  constructor(marker: string, text: string, boundaries: SectionBoundaries | null = null) {
    const { handle, snapshot } = buildModel<SectionWire & { readonly selector: BoundariesWire }>({
      kind: 'section',
      marker: requireText(marker, 'section marker'),
      text: requireText(text, 'section text'),
      boundaries: boundaries === null ? null : boundariesWire(boundaries),
    });
    this.marker = snapshot.marker;
    this.text = snapshot.text;
    this.boundaries = boundaries;
    this.selector =
      boundaries ?? new SectionBoundaries(snapshot.selector.start, snapshot.selector.end);
    attachHandle(this, handle);
    Object.freeze(this);
  }
}

export class StructuredContent {
  readonly kind = 'structured';
  readonly format: DocumentFormat;
  readonly selector: Selector;
  readonly placement: Placement | null;
  readonly #value: NativeValue;

  constructor(
    format: DocumentFormat,
    selector: Selector,
    value: SemanticValue,
    placement: Placement | null = null,
  ) {
    const { handle, snapshot } = buildModel<StructuredWire>({
      kind: 'structured',
      format,
      selector: selectorWire(selector),
      value: toNative(value),
      placement: placement === null ? null : placementWire(placement),
    });
    this.format = snapshot.format;
    this.selector = selector;
    this.#value = snapshot.value;
    this.placement = placement;
    attachHandle(this, handle);
    Object.freeze(this);
  }

  get value(): SemanticValue {
    return fromNative(this.#value);
  }
}

export const structuredFromWire = (wire: StructuredWire): StructuredContent =>
  new StructuredContent(
    wire.format,
    selectorFromWire(wire.selector),
    fromNative(wire.value),
    wire.placement === null ? null : placementFromWire(wire.placement),
  );

export class DocumentSchema {
  readonly format: DocumentFormat;
  readonly key: string;
  readonly value: SemanticScalar;
  readonly #content: StructuredContent;

  constructor(format: DocumentFormat, key: string, value: SemanticScalar) {
    const { handle, snapshot } = buildModel<SchemaWire & { readonly content: StructuredWire }>({
      kind: 'schema',
      format,
      key: requireText(key, 'schema key'),
      value: toNative(value),
    });
    this.format = snapshot.format;
    this.key = snapshot.key;
    this.value = value;
    this.#content = structuredFromWire(snapshot.content);
    attachHandle(this, handle);
    Object.freeze(this);
  }

  content(): StructuredContent {
    return this.#content;
  }
}

export type Content = FileContent | TreeContent | SectionContent | StructuredContent;

export function contentWire(content: Content): ContentWire {
  switch (content.kind) {
    case 'file':
      return { kind: 'file', data: content.data, mode: content.mode };
    case 'tree':
      return { kind: 'tree', entries: content.entries.map(entryToNative) };
    case 'section':
      return {
        kind: 'section',
        marker: content.marker,
        text: content.text,
        boundaries: content.boundaries === null ? null : boundariesWire(content.boundaries),
      };
    case 'structured':
      return {
        kind: 'structured',
        format: content.format,
        selector: selectorWire(content.selector),
        value: toNative(content.value),
        placement: content.placement === null ? null : placementWire(content.placement),
      };
  }
}

export function contentFromWire(wire: ContentWire): Content {
  switch (wire.kind) {
    case 'file':
      return new FileContent(wire.data, wire.mode);
    case 'tree':
      return new TreeContent(wire.entries.map(entryFromNative));
    case 'section':
      return new SectionContent(
        wire.marker,
        wire.text,
        wire.boundaries === null
          ? null
          : new SectionBoundaries(wire.boundaries.start, wire.boundaries.end),
      );
    case 'structured':
      return structuredFromWire(wire);
  }
}

export const treeWire = (tree: TreeContent): TreeWire => ({
  kind: 'tree',
  entries: tree.entries.map(entryToNative),
});

export const treeFromWire = (wire: TreeWire): TreeContent =>
  new TreeContent(wire.entries.map(entryFromNative));
