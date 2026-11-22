import { describe, expect, test } from 'vitest';
import {
  Bundle,
  BundleIdentity,
  Claim,
  Command,
  InstallationTarget,
  InstructionArtifact,
  McpArtifact,
  MemoryStore,
  RenderContext,
  Target,
  ValueError,
  WHOLE,
  assembleTranslations,
  capabilities,
  instructionDestination,
  mcpDestination,
  render,
  renderMany,
  requireTranslatable,
  translateInstructionArtifact,
  translateMcpArtifact,
} from 'flyrail';

const target = Target.project('codex', '/work/app');
const context = new RenderContext(target, 'linux');
const bundle = Bundle.fromArtifacts(new BundleIdentity('demo', '1'), [
  new InstructionArtifact('guide', 'Run the checks.\n'),
]);
const claim = new Claim('file', WHOLE);
const instruction = new InstructionArtifact('guide', 'Run the checks.\n');
const mcp = new McpArtifact('tools', 'tools', new Command(['node', 'server.js']));
const translation = translateInstructionArtifact('demo', instruction, context);

const wrong = (value: unknown): never => value as never;

describe('binding objects of the wrong type throw ValueError', () => {
  test.each([
    ['render bundle', () => render(wrong(target), context), 'bundle must be a Bundle'],
    ['render context', () => render(bundle, wrong(target)), 'context must be a RenderContext'],
    [
      'renderMany context',
      () => renderMany(bundle, [context, wrong({})]),
      'context must be a RenderContext',
    ],
    [
      'RenderContext target',
      () => new RenderContext(wrong(context), 'linux'),
      'target must be a Target',
    ],
    [
      'RenderContext hook runtime',
      () => new RenderContext(target, 'linux', { hookRuntime: wrong(bundle) }),
      'hook runtime must be a HookRuntime',
    ],
    [
      'requireTranslatable',
      () => {
        requireTranslatable(wrong(target));
      },
      'context must be a RenderContext',
    ],
    [
      'instructionDestination',
      () => instructionDestination(wrong(bundle)),
      'context must be a RenderContext',
    ],
    ['mcpDestination', () => mcpDestination(wrong(null)), 'context must be a RenderContext'],
    ['capabilities', () => capabilities(wrong({})), 'context must be a RenderContext'],
    [
      'translateInstructionArtifact artifact',
      () => translateInstructionArtifact('demo', wrong(mcp), context),
      'artifact must be an InstructionArtifact',
    ],
    [
      'translateMcpArtifact artifact',
      () => translateMcpArtifact(wrong(instruction), context, {}),
      'artifact must be an McpArtifact',
    ],
    [
      'assembleTranslations translation',
      () => assembleTranslations([translation, wrong(claim)], [], [context], {}),
      'translation must be an ArtifactTranslation',
    ],
    ['Claim.overlaps', () => claim.overlaps(wrong(instruction)), 'claim must be a Claim'],
  ])('%s', (_label, call, message) => {
    expect(call).toThrow(new ValueError(message));
  });
});

describe('equality with a binding object of another type is false', () => {
  test('Target.equals', () => {
    expect(target.equals(wrong(context))).toBe(false);
    expect(target.equals(Target.project('codex', '/work/app'))).toBe(true);
  });

  test('Claim.equals', () => {
    expect(claim.equals(wrong(instruction))).toBe(false);
    expect(claim.equals(new Claim('file', WHOLE))).toBe(true);
  });

  test('InstallationTarget.equals', () => {
    const store = new MemoryStore();
    const installation: InstallationTarget = context.installation(store);
    expect(installation.equals(wrong(target))).toBe(false);
    expect(installation.equals(context.installation(store))).toBe(true);
  });
});
