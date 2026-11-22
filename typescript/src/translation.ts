import type { InstructionArtifact, McpArtifact } from './artifacts.js';
import type { Bundle } from './bundle.js';
import { callNative } from './internal/call.js';
import { attachHandle, handleOf } from './internal/handles.js';
import { toPairObjects } from './internal/maps.js';
import { modelHandle } from './internal/models.js';
import { requireAbsolute } from './internal/paths.js';
import { requireText } from './internal/text.js';
import type {
  NativeBundle,
  NativeRenderContext,
  NativeRenderedBundle,
  NativeTranslation,
} from './native/binding.js';
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

const contextHandle = (context: RenderContext): NativeRenderContext =>
  handleOf(context) as NativeRenderContext;

export function render(bundle: Bundle, context: RenderContext): RenderedBundle {
  return adoptRenderedBundle(
    callNative(() => native.renderBundle(handleOf(bundle) as NativeBundle, contextHandle(context))),
  );
}

export function renderMany(bundle: Bundle, contexts: readonly RenderContext[]): RenderedBundle {
  return adoptRenderedBundle(
    callNative(() =>
      native.renderBundleMany(handleOf(bundle) as NativeBundle, contexts.map(contextHandle)),
    ),
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
        requireText(bundleId, 'bundle id'),
        modelHandle(artifact),
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
      native.translateMcp(modelHandle(artifact), contextHandle(context), destinations),
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
      translations.map((translation) => handleOf(translation) as NativeTranslation),
      { dependencies: dependencies.map(dependencyWire) },
      contexts.map(contextHandle),
      toPairObjects(routingContext),
    ),
  );
  return adoptRenderedBundle(handle);
}
