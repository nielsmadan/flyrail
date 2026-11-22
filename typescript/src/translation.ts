import { InstructionArtifact, McpArtifact } from './artifacts.js';
import type { Bundle } from './bundle.js';
import { callNative } from './internal/call.js';
import { bundleHandle, contextHandle } from './internal/guards.js';
import { attachHandle, requireHandle } from './internal/handles.js';
import { toPairObjects } from './internal/maps.js';
import { modelHandle } from './internal/models.js';
import { requireAbsolute } from './internal/paths.js';
import { requireBundleId, requireText } from './internal/text.js';
import type { NativeRenderedBundle, NativeTranslation } from './native/binding.js';
import { native } from './native/loader.js';
import type { RenderContext } from './render-context.js';
import {
  adoptRenderedBundle,
  dependencyWire,
  noticeFromWire,
  renderedArtifactFromWire,
  type Dependency,
  type Notice,
  type RenderedArtifact,
  type RenderedBundle,
} from './rendered.js';

export function render(bundle: Bundle, context: RenderContext): RenderedBundle {
  return adoptRenderedBundle(
    callNative(() => native.renderBundle(bundleHandle(bundle), contextHandle(context))),
  );
}

export function renderMany(bundle: Bundle, contexts: readonly RenderContext[]): RenderedBundle {
  return adoptRenderedBundle(
    callNative(() => native.renderBundleMany(bundleHandle(bundle), contexts.map(contextHandle))),
  );
}

let adoptTranslation: (handle: NativeTranslation) => ArtifactTranslation;

export class ArtifactTranslation {
  readonly artifact: RenderedArtifact | null;
  readonly notices: readonly Notice[];
  readonly supported: boolean;

  static {
    adoptTranslation = (handle) => new ArtifactTranslation(handle);
  }

  private constructor(handle: NativeTranslation) {
    const wire = callNative(() => handle.snapshot());
    this.artifact = wire.artifact === null ? null : renderedArtifactFromWire(wire.artifact);
    this.notices = Object.freeze(wire.notices.map(noticeFromWire));
    this.supported = wire.supported;
    attachHandle(this, handle);
    Object.freeze(this);
  }
}

export function translateInstructionArtifact(
  bundleId: string,
  artifact: InstructionArtifact,
  context: RenderContext,
): ArtifactTranslation {
  return adoptTranslation(
    callNative(() =>
      native.translateInstruction(
        requireBundleId(bundleId),
        modelHandle(artifact, InstructionArtifact, 'artifact must be an InstructionArtifact'),
        contextHandle(context),
      ),
    ),
  );
}

export function translateMcpArtifact(
  artifact: McpArtifact,
  context: RenderContext,
  assetDestinations: Readonly<Record<string, string>>,
): ArtifactTranslation {
  const destinations = toPairObjects(assetDestinations).map(({ key, value }) => ({
    key: requireText(key, 'asset id'),
    value: requireAbsolute(value, 'asset destination'),
  }));
  return adoptTranslation(
    callNative(() =>
      native.translateMcp(
        modelHandle(artifact, McpArtifact, 'artifact must be an McpArtifact'),
        contextHandle(context),
        destinations,
      ),
    ),
  );
}

export function assembleTranslations(
  translations: readonly ArtifactTranslation[],
  dependencies: readonly Dependency[],
  contexts: readonly RenderContext[],
  routingContext: Readonly<Record<string, string>>,
): RenderedBundle {
  const handle: NativeRenderedBundle = callNative(() =>
    native.translationsAssemble(
      translations.map((translation) =>
        requireHandle(
          translation,
          ArtifactTranslation,
          native.NativeTranslation,
          'translation must be an ArtifactTranslation',
        ),
      ),
      { dependencies: dependencies.map(dependencyWire) },
      contexts.map(contextHandle),
      toPairObjects(routingContext),
    ),
  );
  return adoptRenderedBundle(handle);
}
