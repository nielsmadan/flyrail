import { describe, expect, test } from 'vitest';
import {
  ArtifactTranslation,
  AssetRef,
  Command,
  Dependency,
  InstructionArtifact,
  McpArtifact,
  ObjectValue,
  RenderContext,
  SectionContent,
  StructuredContent,
  Target,
  ValueError,
  assembleTranslations,
  translateInstructionArtifact,
  translateMcpArtifact,
  valuesEqual,
} from 'flyrail';

const root = '/fixture/translation';
const project = (agent: Parameters<typeof Target.project>[0], options = {}): RenderContext =>
  new RenderContext(Target.project(agent, root), 'linux', options);
const mcp = (id: string, ...argv: string[]): McpArtifact =>
  new McpArtifact(id, 'tools', new Command(argv));

describe('translateInstructionArtifact', () => {
  test('keeps the wire-compatible marker and digest', () => {
    const context = new RenderContext(Target.project('codex', '/fixture/project'), 'linux');
    const translation = translateInstructionArtifact(
      'translation-demo',
      new InstructionArtifact('guide', 'Run the checks.\n'),
      context,
    );
    const content = translation.artifact?.content;
    expect(content).toBeInstanceOf(SectionContent);
    expect((content as SectionContent).marker).toBe(
      '81a6fb0b74cac727508df8661502ff555a19792d232631d282cc3bfcd196dc93',
    );
    const rendered = assembleTranslations([translation], [], [context], context.routingContext);
    expect(rendered.contentDigest).toBe(
      '51ff0633093b74dd5486198a0145530c3883c3f68c30f8140805a66f536f6f8b',
    );
  });

  test('reports activation notices with codes', () => {
    const translation = translateInstructionArtifact(
      'translation-demo',
      new InstructionArtifact('guide', 'Always.'),
      project('codex'),
    );
    expect(translation.supported).toBe(true);
    expect(translation.notices.map((notice) => notice.code)).toEqual([
      'instruction-loading',
      'codex-instruction-discovery',
    ]);
    expect(translation).toBeInstanceOf(ArtifactTranslation);
  });

  test('an unsupported translation has no artifact and one unsupported notice', () => {
    const translation = translateInstructionArtifact(
      'translation-demo',
      new InstructionArtifact('guide', 'Always.'),
      project('cursor', { surface: 'ide', instructionPath: `${root}/custom.mdc` }),
    );
    expect(translation.supported).toBe(false);
    expect(translation.artifact).toBeNull();
    expect(translation.notices).toEqual([
      {
        artifactId: 'guide',
        kind: 'unsupported',
        message: 'Use native content for authored modular rule metadata.',
        code: 'native-instruction-required',
      },
    ]);
  });
});

describe('translateMcpArtifact', () => {
  test('translates a stdio server into a structured claude document', () => {
    const translation = translateMcpArtifact(mcp('tools', 'server'), project('claude'), {});
    expect(translation.artifact?.destination).toBe(`${root}/.mcp.json`);
    expect(translation.artifact?.content).toBeInstanceOf(StructuredContent);
    expect(translation.notices.map((notice) => notice.code)).toEqual(['mcp-activation']);
  });

  test('substitutes the pinned asset revision', () => {
    const artifact = new McpArtifact(
      'tools',
      'tools',
      new Command(['node', new AssetRef('runtime', 'server.js')]),
    );
    const revision = '/fixture/assets/runtime/revisions/sha256-pinned';
    const translation = translateMcpArtifact(artifact, project('codex'), { runtime: revision });
    const content = translation.artifact?.content as StructuredContent;
    expect(translation.artifact?.destination).toBe(`${root}/.codex/config.toml`);
    expect(content.format).toBe('toml');
    expect(
      valuesEqual(
        content.value,
        new ObjectValue([
          ['command', 'node'],
          ['args', [`${revision}/server.js`]],
          ['env', new ObjectValue()],
        ]),
      ),
    ).toBe(true);
  });

  test('an unknown asset reference is refused with the crate text', () => {
    const artifact = new McpArtifact(
      'tools',
      'tools',
      new Command(['node', new AssetRef('runtime', 'server.js')]),
    );
    expect(() => translateMcpArtifact(artifact, project('codex'), {})).toThrow(ValueError);
    expect(() => translateMcpArtifact(artifact, project('codex'), {})).toThrow(
      'unknown translated asset reference',
    );
  });

  test('a relative asset destination is refused', () => {
    expect(() =>
      translateMcpArtifact(mcp('tools', 'server'), project('codex'), { runtime: 'rel' }),
    ).toThrow('asset destination must be an absolute path');
  });

  test('reserved server names are unsupported with the crate code', () => {
    const translation = translateMcpArtifact(
      new McpArtifact('tools', 'workspace', new Command(['server'])),
      project('claude'),
      {},
    );
    expect(translation.notices).toEqual([
      {
        artifactId: 'tools',
        kind: 'unsupported',
        message: 'Claude reserves this MCP server name.',
        code: 'reserved-server-name',
      },
    ]);
  });
});

describe('assembleTranslations', () => {
  const claude = project('claude');
  const translated = (id: string): ArtifactTranslation =>
    translateMcpArtifact(mcp(id, 'server'), claude, {});

  test('preserves dependencies', () => {
    const assembled = assembleTranslations(
      [translated('runtime'), translated('tools')],
      [new Dependency('tools', 'runtime')],
      [claude],
      claude.routingContext,
    );
    expect(assembled.supported).toBe(true);
    expect(
      assembled.dependencies.map((edge) => [edge.dependent, edge.required, edge.mode]),
    ).toEqual([['tools', 'runtime', 'revision']]);
  });

  test.each([
    [[new Dependency('tools', 'missing')], 'dependency references an unknown artifact or asset'],
    [
      [new Dependency('tools', 'runtime'), new Dependency('runtime', 'tools')],
      'cyclic artifact dependencies',
    ],
  ])('a bad dependency set is refused with the crate text', (dependencies, message) => {
    expect(() =>
      assembleTranslations(
        [translated('runtime'), translated('tools')],
        dependencies,
        [claude],
        claude.routingContext,
      ),
    ).toThrow(message);
  });

  test('blocks incompatible claims on one destination', () => {
    const assembled = assembleTranslations(
      [
        translateMcpArtifact(mcp('first', 'first-server'), claude, {}),
        translateMcpArtifact(mcp('second', 'second-server'), claude, {}),
      ],
      [],
      [claude],
      {},
    );
    expect(assembled.supported).toBe(false);
    expect(assembled.artifacts).toEqual([]);
    expect(assembled.notices.at(-1)).toEqual({
      artifactId: 'second',
      kind: 'unsupported',
      message: `Incompatible claims from first and second at ${root}/.mcp.json.`,
      code: 'destination-collision',
    });
  });

  test('identical registrations on one destination coexist', () => {
    const assembled = assembleTranslations(
      [
        translateMcpArtifact(mcp('first', 'first-server'), claude, {}),
        translateMcpArtifact(mcp('compatible', 'first-server'), claude, {}),
      ],
      [],
      [claude],
      {},
    );
    expect(assembled.supported).toBe(true);
    expect(assembled.artifacts).toHaveLength(2);
  });
});
