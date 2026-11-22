import { artifactFromWire, artifactWire, assetFromWire, assetWire } from './artifacts.js';
import type { Artifact, SupportAsset } from './artifacts.js';
import { runReadOnly, type CallOptions } from './internal/abort.js';
import { awaitNative, callNative } from './internal/call.js';
import { toNative } from './internal/convert.js';
import { entryFromNative, entryToNative } from './internal/entries.js';
import { attachHandle, handleOf } from './internal/handles.js';
import { requireAbsolute } from './internal/paths.js';
import { requireText } from './internal/text.js';
import type { ArtifactWire, AssetWire, DependencyWire } from './internal/models.js';
import { dependencyFromWire, dependencyWire, type Dependency } from './rendered.js';
import type {
  JsBundle,
  NativeBundle,
  NativeBundleIdentity,
  NativeSkillSpec,
} from './native/binding.js';
import { native } from './native/loader.js';
import type { SemanticValue } from './values.js';

export interface BundleEntry {
  readonly path: string;
  readonly kind: 'file' | 'directory';
  readonly data: Uint8Array | null;
  readonly executable: boolean;
}

export function fileEntry(
  path: string,
  data: Uint8Array,
  options?: { readonly executable?: boolean },
): BundleEntry {
  return Object.freeze({
    path,
    kind: 'file',
    data: new Uint8Array(data),
    executable: options?.executable ?? false,
  });
}

export function directoryEntry(path: string): BundleEntry {
  return Object.freeze({ path, kind: 'directory', data: null, executable: false });
}

interface BundleModels {
  readonly artifacts: readonly Artifact[];
  readonly assets: readonly SupportAsset[];
  readonly dependencies: readonly Dependency[];
}

export class BundleIdentity {
  readonly id: string;
  readonly version: string;

  constructor(id: string, version: string) {
    const handle: NativeBundleIdentity = callNative(() =>
      native.bundleIdentity(requireText(id, 'bundle id'), requireText(version, 'bundle version')),
    );
    const snapshot = handle.snapshot();
    this.id = snapshot.id;
    this.version = snapshot.version;
    attachHandle(this, handle);
    Object.freeze(this);
  }

  equals(other: BundleIdentity): boolean {
    return this.id === other.id && this.version === other.version;
  }
}

export class SkillSpec {
  readonly name: string;
  readonly path: string;
  readonly executables: readonly string[];

  constructor(name: string, path: string, executables: readonly string[] = []) {
    const handle: NativeSkillSpec = callNative(() =>
      native.bundleSkillSpec(
        requireText(name, 'skill name'),
        requireText(path, 'skill path'),
        executables.map((executable) => requireText(executable, 'executable path')),
      ),
    );
    const snapshot = handle.snapshot();
    this.name = snapshot.name;
    this.path = snapshot.path;
    this.executables = Object.freeze([...snapshot.executables]);
    attachHandle(this, handle);
    Object.freeze(this);
  }
}

export class Bundle {
  readonly identity: BundleIdentity;
  readonly id: string;
  readonly version: string;
  readonly skills: readonly SkillSpec[];
  readonly entries: readonly BundleEntry[];
  readonly contentDigest: string;
  readonly sourceRoots: readonly string[];
  readonly schemaVersion: number;
  #models: BundleModels | null = null;

  private constructor(handle: NativeBundle) {
    const snapshot: JsBundle = handle.snapshot();
    this.identity = new BundleIdentity(snapshot.id, snapshot.version);
    this.id = snapshot.id;
    this.version = snapshot.version;
    this.skills = Object.freeze(
      snapshot.skills.map((skill) => new SkillSpec(skill.name, skill.path, skill.executables)),
    );
    this.entries = Object.freeze(snapshot.entries.map(entryFromNative));
    this.contentDigest = snapshot.contentDigest;
    this.sourceRoots = Object.freeze([...snapshot.sourceRoots]);
    this.schemaVersion = snapshot.schemaVersion;
    attachHandle(this, handle);
    Object.freeze(this);
  }

  static fromArtifacts(
    identity: BundleIdentity,
    artifacts: readonly Artifact[],
    assets: readonly SupportAsset[] = [],
    dependencies: readonly Dependency[] = [],
  ): Bundle {
    return new Bundle(
      callNative(() =>
        native.bundleFromArtifacts(identity.id, identity.version, {
          artifacts: artifacts.map(artifactWire),
          assets: assets.map(assetWire),
          dependencies: dependencies.map(dependencyWire),
        }),
      ),
    );
  }

  get artifacts(): readonly Artifact[] {
    return this.#loadModels().artifacts;
  }

  get assets(): readonly SupportAsset[] {
    return this.#loadModels().assets;
  }

  get dependencies(): readonly Dependency[] {
    return this.#loadModels().dependencies;
  }

  #loadModels(): BundleModels {
    this.#models ??= (() => {
      const wires = callNative(() => (handleOf(this) as NativeBundle).models());
      return {
        artifacts: Object.freeze(
          wires.artifacts.map((wire) => artifactFromWire(wire as ArtifactWire)),
        ),
        assets: Object.freeze(wires.assets.map((wire) => assetFromWire(wire as AssetWire))),
        dependencies: Object.freeze(
          wires.dependencies.map((wire) => dependencyFromWire(wire as DependencyWire)),
        ),
      };
    })();
    return this.#models;
  }

  static fromDirectory(root: string, options?: CallOptions): Promise<Bundle> {
    return runReadOnly(options?.signal, async () => {
      const handle = await awaitNative(
        native.bundleFromDirectory(requireAbsolute(root, 'bundle root')),
      );
      return new Bundle(handle);
    });
  }

  static fromZip(path: string, resource: string, options?: CallOptions): Promise<Bundle> {
    return runReadOnly(options?.signal, async () => {
      const handle = await awaitNative(
        native.bundleFromZip(
          requireAbsolute(path, 'archive path'),
          requireText(resource, 'archive resource'),
        ),
      );
      return new Bundle(handle);
    });
  }

  static fromMemory(manifest: Uint8Array, entries: readonly BundleEntry[]): Bundle {
    return new Bundle(
      callNative(() => native.bundleFromMemory(manifest, entries.map(entryToNative))),
    );
  }

  static fromMemoryValue(manifest: SemanticValue, entries: readonly BundleEntry[]): Bundle {
    return new Bundle(
      callNative(() =>
        native.bundleFromMemoryValue(toNative(manifest), entries.map(entryToNative)),
      ),
    );
  }
}
