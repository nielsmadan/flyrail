import { describe, expect, test } from 'vitest';
import {
  HookRuntime,
  RenderContext,
  TRANSLATABLE_AGENTS,
  Target,
  UnsupportedTranslationError,
  ValueError,
  capabilities,
  instructionDestination,
  isTranslatable,
  mcpDestination,
  requireTranslatable,
  untranslatableMessage,
} from 'flyrail';

const project = (agent: Parameters<typeof Target.project>[0], root = '/work/app'): RenderContext =>
  new RenderContext(Target.project(agent, root), 'macos');

describe('destinations', () => {
  test('the codex project instruction destination is AGENTS.md in the project root', () => {
    expect(instructionDestination(project('codex'))).toBe('/work/app/AGENTS.md');
  });

  test('the claude project MCP destination is .mcp.json in the project root', () => {
    expect(mcpDestination(project('claude'))).toBe('/work/app/.mcp.json');
  });

  test('a relocated claude user target has no MCP destination until a path is given', () => {
    const target = Target.user('claude', '/home/u', { CLAUDE_CONFIG_DIR: '~/selected' });
    expect(instructionDestination(new RenderContext(target, 'linux'))).toBe(
      '/home/u/selected/CLAUDE.md',
    );
    expect(mcpDestination(new RenderContext(target, 'linux'))).toBeNull();
    expect(
      mcpDestination(new RenderContext(target, 'linux', { mcpPath: '/home/u/chosen.json' })),
    ).toBe('/home/u/chosen.json');
  });

  test('a relative CODEX_HOME is refused with the crate text', () => {
    const target = Target.user('codex', '/home/u', { CODEX_HOME: 'relative' });
    expect(() => instructionDestination(new RenderContext(target, 'linux'))).toThrow(ValueError);
    expect(() => instructionDestination(new RenderContext(target, 'linux'))).toThrow(
      'environment paths must be absolute or start with ~/',
    );
  });

  test('droid has no translation but keeps explicit paths', () => {
    const context = new RenderContext(Target.project('droid', '/work/app'), 'linux', {
      instructionPath: '/work/app/instructions.md',
    });
    expect(instructionDestination(context)).toBe('/work/app/instructions.md');
    expect(mcpDestination(context)).toBeNull();
  });
});

describe('translatable agents', () => {
  test('lists every agent except droid', () => {
    expect(TRANSLATABLE_AGENTS).toEqual(['claude', 'codex', 'opencode', 'pi', 'cursor', 'copilot']);
    expect(Object.isFrozen(TRANSLATABLE_AGENTS)).toBe(true);
  });

  test('isTranslatable follows the list', () => {
    expect(isTranslatable('codex')).toBe(true);
    expect(isTranslatable('droid')).toBe(false);
  });

  test('untranslatableMessage names the agent', () => {
    expect(untranslatableMessage('droid')).toBe(
      'Flyrail detects droid but has no verified configuration translation.',
    );
  });

  test('requireTranslatable throws the crate code for droid and passes for codex', () => {
    requireTranslatable(project('codex'));
    expect(isTranslatable('codex')).toBe(true);
    let failure: unknown;
    try {
      requireTranslatable(project('droid'));
    } catch (error) {
      failure = error;
    }
    expect(failure).toBeInstanceOf(UnsupportedTranslationError);
    expect((failure as UnsupportedTranslationError).code).toBe('agent-unsupported');
    expect((failure as UnsupportedTranslationError).message).toBe(untranslatableMessage('droid'));
  });
});

describe('capabilities', () => {
  test('pi lists one capability per family with the crate wording', () => {
    const runtime = new HookRuntime({ node: '/opt/node/bin/node', shell: 'posix' });
    const context = new RenderContext(Target.project('pi', '/fixture/project'), 'macos', {
      hookRuntime: runtime,
    });
    expect(capabilities(context)).toEqual([
      { family: 'skills', portable: true, native: true, limitations: [], prerequisites: [] },
      {
        family: 'instructions',
        portable: true,
        native: true,
        limitations: [
          'Discovery, overrides, byte limits and session loading remain host-controlled.',
        ],
        prerequisites: [],
      },
      {
        family: 'mcp',
        portable: true,
        native: true,
        limitations: [
          'Transport, interpolation, environment references and cwd are checked per request.',
        ],
        prerequisites: ['pi-mcp-adapter@2.32.1'],
      },
      {
        family: 'hooks',
        portable: true,
        native: true,
        limitations: ['Event/outcome and native shell support are validated for each request.'],
        prerequisites: [
          'Node 24+; explicit executable for native hosts. See the hook protocol matrix.',
        ],
      },
    ]);
  });

  test('droid reports no portable or native support', () => {
    expect(capabilities(project('droid')).map((item) => [item.portable, item.native])).toEqual([
      [false, false],
      [false, false],
      [false, false],
      [false, false],
    ]);
  });
});
