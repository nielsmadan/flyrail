import type { Agent, Family } from './enums.js';
import { callNative } from './internal/call.js';
import { contextHandle } from './internal/guards.js';
import { native } from './native/loader.js';
import type { RenderContext } from './render-context.js';
import { frozenCopy } from './internal/results.js';

export interface Capability {
  readonly family: Family;
  readonly portable: boolean;
  readonly native: boolean;
  readonly limitations: readonly string[];
  readonly prerequisites: readonly string[];
}

export const TRANSLATABLE_AGENTS: readonly Agent[] = Object.freeze(
  callNative(() => native.renderTranslatableAgents()) as Agent[],
);

export function isTranslatable(agent: Agent): boolean {
  return callNative(() => native.renderIsTranslatable(agent));
}

export function untranslatableMessage(agent: Agent): string {
  return callNative(() => native.renderUntranslatableMessage(agent));
}

export function requireTranslatable(context: RenderContext): void {
  callNative(() => native.renderRequireTranslatable(contextHandle(context)));
}

export function instructionDestination(context: RenderContext): string | null {
  return callNative(() => native.renderInstructionDestination(contextHandle(context)));
}

export function mcpDestination(context: RenderContext): string | null {
  return callNative(() => native.renderMcpDestination(contextHandle(context)));
}

export function capabilities(context: RenderContext): readonly Capability[] {
  return Object.freeze(
    callNative(() => native.renderCapabilities(contextHandle(context))).map((capability) =>
      Object.freeze({
        family: capability.family as Family,
        portable: capability.portable,
        native: capability.native,
        limitations: frozenCopy(capability.limitations),
        prerequisites: frozenCopy(capability.prerequisites),
      }),
    ),
  );
}
