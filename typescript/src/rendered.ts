import { contentFromWire, contentWire, DocumentSchema } from './content.js';
import type { Content } from './content.js';
import type { DependencyMode, Family, NoticeKind } from './enums.js';
import { callNative } from './internal/call.js';
import { fromNative, toNative } from './internal/convert.js';
import { attachHandle } from './internal/handles.js';
import { fromPairObjects, toPairObjects } from './internal/maps.js';
import {
  buildModel,
  type DependencyWire,
  type NoticeWire,
  type RenderedArtifactWire,
  type RenderedBundleWire,
} from './internal/models.js';
import { requireText } from './internal/text.js';
import type { NativeRenderedBundle } from './native/binding.js';
import { native } from './native/loader.js';
import type { SemanticScalar } from './values.js';

export class Dependency {
  readonly dependent: string;
  readonly required: string;
  readonly mode: DependencyMode;

  constructor(dependent: string, required: string, mode: DependencyMode = 'revision') {
    const { handle, snapshot } = buildModel<DependencyWire>({
      kind: 'dependency',
      dependent: requireText(dependent, 'dependent artifact'),
      required: requireText(required, 'required artifact'),
      mode,
    });
    this.dependent = snapshot.dependent;
    this.required = snapshot.required;
    this.mode = snapshot.mode;
    attachHandle(this, handle);
    Object.freeze(this);
  }
}

export const dependencyWire = (dependency: Dependency): DependencyWire => ({
  kind: 'dependency',
  dependent: dependency.dependent,
  required: dependency.required,
  mode: dependency.mode,
});

export const dependencyFromWire = (wire: DependencyWire): Dependency =>
  new Dependency(wire.dependent, wire.required, wire.mode);

export function dependencyOrder(
  names: readonly string[],
  dependencies: readonly Dependency[],
): readonly string[] {
  const order = callNative(() =>
    native.modelsDependencyOrder(
      names.map((name) => requireText(name, 'artifact name')),
      { dependencies: dependencies.map(dependencyWire) },
    ),
  );
  return Object.freeze(order);
}

export interface Notice {
  readonly artifactId: string;
  readonly kind: NoticeKind;
  readonly message: string;
  readonly code: string | null;
}

export const noticeWire = (notice: Notice): NoticeWire => ({
  kind: 'notice',
  artifactId: requireText(notice.artifactId, 'notice artifact id'),
  noticeKind: notice.kind,
  message: requireText(notice.message, 'notice message'),
  code: notice.code === null ? null : requireText(notice.code, 'notice code'),
});

export const noticeFromWire = (wire: NoticeWire): Notice =>
  Object.freeze({
    artifactId: wire.artifactId,
    kind: wire.noticeKind,
    message: wire.message,
    code: wire.code,
  });

export interface RenderedArtifactInit {
  readonly id: string;
  readonly family: Family;
  readonly destination: string;
  readonly content: Content;
  readonly subtree?: string | null;
  readonly requireExisting?: boolean;
  readonly schemaRequirements?: readonly DocumentSchema[];
}

export class RenderedArtifact {
  readonly id: string;
  readonly family: Family;
  readonly destination: string;
  readonly content: Content;
  readonly subtree: string | null;
  readonly requireExisting: boolean;
  readonly schemaRequirements: readonly DocumentSchema[];

  constructor(init: RenderedArtifactInit) {
    const wire = callNative(() =>
      native.renderedArtifactBuild({
        kind: 'rendered-artifact',
        id: requireText(init.id, 'rendered artifact id'),
        family: init.family,
        destination: requireText(init.destination, 'rendered destination'),
        content: contentWire(init.content),
        subtree:
          init.subtree === undefined || init.subtree === null
            ? null
            : requireText(init.subtree, 'rendered subtree'),
        requireExisting: init.requireExisting ?? false,
        schemaRequirements: (init.schemaRequirements ?? []).map((schema) => ({
          kind: 'schema',
          format: schema.format,
          key: schema.key,
          value: toNative(schema.value),
        })),
      }),
    );
    this.id = wire.id;
    this.family = wire.family;
    this.destination = wire.destination;
    this.content = contentFromWire(wire.content);
    this.subtree = wire.subtree;
    this.requireExisting = wire.requireExisting;
    this.schemaRequirements = Object.freeze(
      wire.schemaRequirements.map(
        (schema) =>
          new DocumentSchema(schema.format, schema.key, fromNative(schema.value) as SemanticScalar),
      ),
    );
    Object.freeze(this);
  }
}

export const renderedArtifactFromWire = (wire: RenderedArtifactWire): RenderedArtifact =>
  new RenderedArtifact({
    id: wire.id,
    family: wire.family,
    destination: wire.destination,
    content: contentFromWire(wire.content),
    subtree: wire.subtree,
    requireExisting: wire.requireExisting,
    schemaRequirements: wire.schemaRequirements.map(
      (schema) =>
        new DocumentSchema(schema.format, schema.key, fromNative(schema.value) as SemanticScalar),
    ),
  });

let adoptable: NativeRenderedBundle | null = null;

export class RenderedBundle {
  readonly artifacts: readonly RenderedArtifact[];
  readonly dependencies: readonly Dependency[];
  readonly notices: readonly Notice[];
  readonly supported: boolean;
  readonly contentDigest: string;
  readonly routingContext: Readonly<Record<string, string>>;

  constructor(
    artifacts: readonly RenderedArtifact[],
    dependencies: readonly Dependency[] = [],
    notices: readonly Notice[] = [],
    routingContext?: Readonly<Record<string, string>>,
  ) {
    const handle: NativeRenderedBundle =
      adoptable ??
      callNative(() =>
        native.renderedBundleBuild({
          artifacts: artifacts.map(renderedArtifactWire),
          dependencies: dependencies.map(dependencyWire),
          notices: notices.map(noticeWire),
          routingContext: toPairObjects(routingContext),
        }),
      );
    adoptable = null;
    const wire: RenderedBundleWire = callNative(() => handle.snapshot());
    this.artifacts = Object.freeze(wire.artifacts.map(renderedArtifactFromWire));
    this.dependencies = Object.freeze(wire.dependencies.map(dependencyFromWire));
    this.notices = Object.freeze(wire.notices.map(noticeFromWire));
    this.supported = wire.supported;
    this.contentDigest = wire.contentDigest;
    this.routingContext = fromPairObjects(wire.routingContext);
    attachHandle(this, handle);
    Object.freeze(this);
  }

  static empty(): RenderedBundle {
    return adoptRenderedBundle(callNative(() => native.renderedBundleEmpty()));
  }
}

export function adoptRenderedBundle(handle: NativeRenderedBundle): RenderedBundle {
  adoptable = handle;
  try {
    return new RenderedBundle([]);
  } finally {
    adoptable = null;
  }
}

const renderedArtifactWire = (artifact: RenderedArtifact): RenderedArtifactWire => ({
  kind: 'rendered-artifact',
  id: artifact.id,
  family: artifact.family,
  destination: artifact.destination,
  content: contentWire(artifact.content),
  subtree: artifact.subtree,
  requireExisting: artifact.requireExisting,
  schemaRequirements: artifact.schemaRequirements.map((schema) => ({
    kind: 'schema',
    format: schema.format,
    key: schema.key,
    value: toNative(schema.value),
  })),
});
