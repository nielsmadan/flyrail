import { Audience } from './artifacts.js';
import type { Agent, Platform, Surface, TargetScope } from './enums.js';
import type { HookRuntime } from './hook-runtime.js';
import { adoptInstallationTarget, contextPairs, type InstallationTarget } from './installation.js';
import { callNative } from './internal/call.js';
import { runtimeHandle, targetHandle } from './internal/guards.js';
import { attachHandle, handleOf } from './internal/handles.js';
import { fromPairObjects } from './internal/maps.js';
import { optionalAbsolute } from './internal/paths.js';
import type { NativeRenderContext } from './native/binding.js';
import { native } from './native/loader.js';
import { storeHandle, type StateStore } from './stores.js';
import type { Target } from './targets.js';

export interface RenderOptions {
  readonly surface?: Surface;
  readonly executionRoot?: string | null;
  readonly instructionPath?: string | null;
  readonly mcpPath?: string | null;
  readonly nativeRoot?: string | null;
  readonly assetRoot?: string | null;
  readonly hookRuntime?: HookRuntime | null;
}

const present = (key: string, value: string | null): Record<string, string> =>
  value === null ? {} : { [key]: value };

export class RenderContext {
  readonly target: Target;
  readonly platform: Platform;
  readonly surface: Surface;
  readonly root: string;
  readonly audience: Audience;
  readonly skills: string;
  readonly executionRoot: string | null;
  readonly instructionPath: string | null;
  readonly mcpPath: string | null;
  readonly nativeRoot: string | null;
  readonly assetRoot: string | null;
  readonly hookRuntime: HookRuntime | null;
  readonly routingContext: Readonly<Record<string, string>>;

  constructor(target: Target, platform: Platform, options?: RenderOptions) {
    const runtime = options?.hookRuntime ?? null;
    const handle: NativeRenderContext = callNative(() =>
      native.renderContextNew(
        targetHandle(target),
        platform,
        options?.surface ?? 'cli',
        {
          ...present('executionRoot', optionalAbsolute(options?.executionRoot, 'execution root')),
          ...present(
            'instructionPath',
            optionalAbsolute(options?.instructionPath, 'instruction path'),
          ),
          ...present('mcpPath', optionalAbsolute(options?.mcpPath, 'MCP path')),
          ...present('nativeRoot', optionalAbsolute(options?.nativeRoot, 'native root')),
          ...present('assetRoot', optionalAbsolute(options?.assetRoot, 'asset root')),
        },
        runtimeHandle(runtime),
      ),
    );
    const snapshot = handle.snapshot();
    this.target = target;
    this.platform = snapshot.platform as Platform;
    this.surface = snapshot.surface as Surface;
    this.root = snapshot.root;
    this.audience = new Audience(
      snapshot.agent as Agent,
      snapshot.scope as TargetScope,
      this.surface,
      this.platform,
    );
    this.skills = snapshot.skills;
    this.executionRoot = snapshot.executionRoot ?? null;
    this.instructionPath = snapshot.instructionPath ?? null;
    this.mcpPath = snapshot.mcpPath ?? null;
    this.nativeRoot = snapshot.nativeRoot ?? null;
    this.assetRoot = snapshot.assetRoot ?? null;
    this.hookRuntime = runtime;
    this.routingContext = fromPairObjects(snapshot.routingContext);
    attachHandle(this, handle);
    Object.freeze(this);
  }

  installation(
    store: StateStore,
    context?: Readonly<Record<string, string>> | null,
  ): InstallationTarget {
    const pairs =
      context === undefined || context === null ? undefined : contextPairs(context, 'context');
    return adoptInstallationTarget(
      store,
      callNative(() =>
        native.renderContextInstallation(
          handleOf(this) as NativeRenderContext,
          storeHandle(store),
          pairs,
        ),
      ),
    );
  }
}
