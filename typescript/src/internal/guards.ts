import { Bundle } from '../bundle.js';
import { HookRuntime } from '../hook-runtime.js';
import type {
  NativeBundle,
  NativeHookRuntime,
  NativeRenderContext,
  NativeRenderedBundle,
  NativeTarget,
} from '../native/binding.js';
import { native } from '../native/loader.js';
import { RenderContext } from '../render-context.js';
import { RenderedBundle } from '../rendered.js';
import { Target } from '../targets.js';
import { requireHandle } from './handles.js';

export const bundleHandle = (bundle: Bundle): NativeBundle =>
  requireHandle(bundle, Bundle, native.NativeBundle, 'bundle must be a Bundle');

export const renderedHandle = (rendered: RenderedBundle): NativeRenderedBundle =>
  requireHandle(
    rendered,
    RenderedBundle,
    native.NativeRenderedBundle,
    'rendered bundle must be a RenderedBundle',
  );

export const targetHandle = (target: Target): NativeTarget =>
  requireHandle(target, Target, native.NativeTarget, 'target must be a Target');

export const contextHandle = (context: RenderContext): NativeRenderContext =>
  requireHandle(
    context,
    RenderContext,
    native.NativeRenderContext,
    'context must be a RenderContext',
  );

export const runtimeHandle = (
  runtime: HookRuntime | null | undefined,
): NativeHookRuntime | undefined =>
  runtime === null || runtime === undefined
    ? undefined
    : requireHandle(
        runtime,
        HookRuntime,
        native.NativeHookRuntime,
        'hook runtime must be a HookRuntime',
      );
