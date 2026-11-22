import { describe, expect, test } from 'vitest';
import {
  AssetRef,
  Audience,
  Bundle,
  BundleIdentity,
  Command,
  Dependency,
  EnvRef,
  FileContent,
  HookArtifact,
  HttpTransport,
  InstructionArtifact,
  McpArtifact,
  NativeArtifact,
  SectionContent,
  SkillArtifact,
  SupportAsset,
  TreeContent,
  ValueError,
  dependencyOrder,
  fileEntry,
} from 'flyrail';

const skillTree = (): TreeContent =>
  new TreeContent([fileEntry('SKILL.md', new TextEncoder().encode('# skill'))]);

describe('artifact constructors', () => {
  test('an instruction keeps its text', () => {
    expect(new InstructionArtifact('guide', 'text').text).toBe('text');
  });

  test('a hook with a zero timeout is refused with the Rust text', () => {
    expect(
      () =>
        new HookArtifact({
          id: 'check',
          event: 'before-tool',
          command: new Command(['node']),
          timeoutMs: 0,
          outcomes: ['continue'],
        }),
    ).toThrow(new ValueError('hook timeout must be 1-300000 integer milliseconds'));
  });

  test('a hook sorts outcomes and tools and reports its protocol version', () => {
    const hook = new HookArtifact({
      id: 'check',
      event: 'before-tool',
      command: new Command(['node', 'check.js']),
      timeoutMs: 500,
      outcomes: ['continue', 'block'],
      tools: ['write', 'edit'],
    });
    expect(hook.outcomes).toEqual(['block', 'continue']);
    expect(hook.tools).toEqual(['edit', 'write']);
    expect(hook.protocolVersion).toBe(1);
  });

  test('an empty command is refused', () => {
    expect(() => new Command([], {})).toThrow(new ValueError('argv requires a command'));
  });

  test('a command keeps asset references and sorts its environment', () => {
    const command = new Command(['run', new AssetRef('helper', 'bin/run')], {
      ZED: 'z',
      ALPHA: new EnvRef('HOME'),
    });
    expect(Object.keys(command.env)).toEqual(['ALPHA', 'ZED']);
    expect(command.argv[1]).toBeInstanceOf(AssetRef);
    expect((command.argv[1] as AssetRef).path).toBe('bin/run');
    expect((command.env.ALPHA as EnvRef).name).toBe('HOME');
  });

  test('an MCP artifact carries an HTTP transport with a bearer reference', () => {
    const artifact = new McpArtifact(
      'server',
      'review',
      new HttpTransport('https://example.com/mcp', new EnvRef('TOKEN')),
    );
    const transport = artifact.transport as HttpTransport;
    expect(transport.url).toBe('https://example.com/mcp');
    expect(transport.bearerToken?.name).toBe('TOKEN');
  });

  test('a native artifact refuses a directory-scope audience', () => {
    expect(() => new Audience('codex', 'directory', 'cli')).toThrow(
      new ValueError('native audiences require project or user scope'),
    );
  });

  test('a native artifact holds its content', () => {
    const artifact = new NativeArtifact(
      'note',
      'instructions',
      new Audience('codex', 'project', 'cli'),
      'AGENTS.md',
      new SectionContent('guide', 'x'),
    );
    expect(artifact.content).toBeInstanceOf(SectionContent);
    expect(artifact.audience.agent).toBe('codex');
  });
});

describe('Bundle.fromArtifacts', () => {
  test('builds a bundle whose artifacts are rebuilt as classes', () => {
    const bundle = Bundle.fromArtifacts(new BundleIdentity('team', '1'), [
      new InstructionArtifact('guide', 'text'),
    ]);
    expect(bundle.artifacts[0]).toBeInstanceOf(InstructionArtifact);
    expect(bundle.contentDigest).toMatch(/^[0-9a-f]{64}$/);
    expect(bundle.assets).toEqual([]);
    expect(bundle.dependencies).toEqual([]);
  });

  test('carries skills, assets and dependencies through the native bundle', () => {
    const bundle = Bundle.fromArtifacts(
      new BundleIdentity('team', '1'),
      [new SkillArtifact('review', 'review', skillTree())],
      [new SupportAsset('helper', 'skills', skillTree())],
      [new Dependency('review', 'helper', 'stable-reference')],
    );
    expect(bundle.artifacts[0]).toBeInstanceOf(SkillArtifact);
    expect(bundle.assets[0]?.id).toBe('helper');
    expect(bundle.dependencies[0]?.mode).toBe('stable-reference');
  });
});

describe('dependencyOrder', () => {
  test('orders a chain with dependencies first', () => {
    expect(
      dependencyOrder(['c', 'b', 'a'], [new Dependency('c', 'b'), new Dependency('b', 'a')]),
    ).toEqual(['a', 'b', 'c']);
  });

  test('refuses a cycle', () => {
    expect(() =>
      dependencyOrder(['a', 'b'], [new Dependency('a', 'b'), new Dependency('b', 'a')]),
    ).toThrow(ValueError);
  });
});

describe('artifact isolation and dispatch', () => {
  test('mutating the bytes of an artifact content does not change the bundle digest', () => {
    const artifact = new NativeArtifact(
      'a',
      'instructions',
      new Audience('codex', 'project', 'cli'),
      'AGENTS.md',
      new FileContent(new Uint8Array([1, 2]), 'private'),
    );
    const identity = new BundleIdentity('team', '1');
    const before = Bundle.fromArtifacts(identity, [artifact]).contentDigest;
    (artifact.content as FileContent).data[0] = 9;
    expect(Bundle.fromArtifacts(identity, [artifact]).contentDigest).toBe(before);
  });

  test('an MCP artifact dispatches its transport on kind', () => {
    const transport = { kind: 'http', url: 'https://example.test/mcp', bearerToken: null };
    const artifact = new McpArtifact('m', 'm', transport as unknown as HttpTransport);
    expect((artifact.transport as HttpTransport).url).toBe('https://example.test/mcp');
  });

  test('a hook with a fractional timeout reports the crate range message', () => {
    expect(
      () =>
        new HookArtifact({
          id: 'h',
          event: 'stop',
          command: new Command(['x']),
          timeoutMs: 1.5,
          outcomes: ['continue'],
        }),
    ).toThrow('hook timeout must be 1-300000 integer milliseconds');
  });
});
