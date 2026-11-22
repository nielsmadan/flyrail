import { describe, expect, test } from 'vitest';
import {
  Bundle,
  BundleIdentity,
  Command,
  Dependency,
  DocumentSchema,
  FileContent,
  HookArtifact,
  HookRuntime,
  InstructionArtifact,
  RenderContext,
  RenderedArtifact,
  RenderedBundle,
  SectionContent,
  StructuredContent,
  Selector,
  Key,
  Target,
  ValueError,
  render,
  renderMany,
} from 'flyrail';

const instructionBundle = (): Bundle =>
  Bundle.fromArtifacts(new BundleIdentity('demo', '1'), [
    new InstructionArtifact('guide', 'Run the checks.\n'),
  ]);

const codex = (root = '/work/app', options = {}): RenderContext =>
  new RenderContext(Target.project('codex', root), 'linux', options);

describe('HookRuntime', () => {
  test('a relative node path is refused', () => {
    expect(() => new HookRuntime({ node: 'node' })).toThrow(ValueError);
  });

  test('exposes the embedded runner resources', () => {
    const runtime = new HookRuntime({ node: '/usr/bin/node' });
    expect(runtime.node).toBe('/usr/bin/node');
    expect(runtime.resources.map((entry) => entry.path)).toContain('runner.mts');
  });

  test('copies resource data on every read', () => {
    const runtime = new HookRuntime();
    const first = runtime.resources[0];
    first?.data?.fill(0);
    expect(runtime.resources[0]?.data?.some((byte) => byte !== 0)).toBe(true);
  });

  test('keeps the shell it was given', () => {
    expect(new HookRuntime({ shell: 'cmd' }).shell).toBe('cmd');
    expect(() => new HookRuntime({ shell: 'fish' as 'cmd' })).toThrow(
      "'fish' is not a valid HookShell",
    );
  });
});

describe('RenderContext', () => {
  test('exposes the target, audience and derived roots', () => {
    const context = codex('/work/app', { executionRoot: '/work/run' });
    expect(context.root).toBe('/work/app');
    expect(context.skills).toBe('/work/app/.agents/skills');
    expect(context.executionRoot).toBe('/work/run');
    expect(context.surface).toBe('cli');
    expect(context.audience.agent).toBe('codex');
    expect(context.audience.platform).toBe('linux');
    expect(context.routingContext.agent).toBe('codex');
    expect(context.routingContext.root).toBe('/work/app');
  });

  test('a directory target cannot make a context', () => {
    expect(() => new RenderContext(Target.directory('/work/skills'), 'linux')).toThrow(
      'render context requires a project or user agent Target',
    );
  });

  test('a relative option path is refused', () => {
    expect(() => codex('/work/app', { mcpPath: 'mcp.json' })).toThrow(
      'MCP path must be an absolute path',
    );
  });
});

describe('render', () => {
  test('renders an instruction bundle into AGENTS.md for codex', () => {
    const rendered = render(instructionBundle(), codex());
    expect(rendered.supported).toBe(true);
    expect(rendered.artifacts.map((artifact) => artifact.destination)).toEqual([
      '/work/app/AGENTS.md',
    ]);
    expect(rendered.artifacts[0]?.content).toBeInstanceOf(SectionContent);
    expect(rendered.contentDigest).toMatch(/^[0-9a-f]{64}$/);
  });

  test('a hook bundle without a hook runtime is unsupported with an unsupported notice', () => {
    const bundle = Bundle.fromArtifacts(new BundleIdentity('demo', '1'), [
      new HookArtifact({
        id: 'start',
        event: 'session-start',
        command: new Command(['/bin/echo']),
        timeoutMs: 1000,
        outcomes: ['continue'],
      }),
    ]);
    const rendered = render(bundle, codex());
    expect(rendered.supported).toBe(false);
    expect(rendered.artifacts).toEqual([]);
    expect(rendered.notices.map((notice) => notice.kind)).toContain('unsupported');
  });

  test('a hook bundle with a hook runtime renders support files', () => {
    const bundle = Bundle.fromArtifacts(new BundleIdentity('demo', '1'), [
      new HookArtifact({
        id: 'start',
        event: 'session-start',
        command: new Command(['/bin/echo']),
        timeoutMs: 1000,
        outcomes: ['continue'],
      }),
    ]);
    const rendered = render(
      bundle,
      codex('/work/app', {
        hookRuntime: new HookRuntime({ node: '/opt/node/bin/node', shell: 'posix' }),
      }),
    );
    expect(rendered.supported).toBe(true);
    expect(rendered.artifacts.length).toBeGreaterThan(1);
  });

  test('renderMany with no contexts is refused with the crate text', () => {
    expect(() => renderMany(instructionBundle(), [])).toThrow(ValueError);
    expect(() => renderMany(instructionBundle(), [])).toThrow(
      'render_many requires at least one explicit context',
    );
  });

  test('renderMany over one context renders the same destinations as render', () => {
    const context = codex();
    expect(
      renderMany(instructionBundle(), [context]).artifacts.map((artifact) => artifact.destination),
    ).toEqual(
      render(instructionBundle(), context).artifacts.map((artifact) => artifact.destination),
    );
  });

  test('renderMany is independent of context order', () => {
    const contexts = [codex(), new RenderContext(Target.project('claude', '/work/app'), 'linux')];
    const forward = renderMany(instructionBundle(), contexts);
    const reverse = renderMany(instructionBundle(), [...contexts].reverse());
    expect(forward.contentDigest).toBe(reverse.contentDigest);
    expect(forward.artifacts.map((artifact) => artifact.destination)).toEqual(
      reverse.artifacts.map((artifact) => artifact.destination),
    );
  });
});

describe('RenderedArtifact', () => {
  const section = (): SectionContent => new SectionContent('guide', 'Hello');

  test('keeps its fields', () => {
    const artifact = new RenderedArtifact({
      id: 'guide',
      family: 'instructions',
      destination: '/a/AGENTS.md',
      content: section(),
    });
    expect(artifact.subtree).toBeNull();
    expect(artifact.requireExisting).toBe(false);
    expect(artifact.schemaRequirements).toEqual([]);
    expect(artifact.content).toBeInstanceOf(SectionContent);
  });

  test.each([
    [
      'a bad id',
      { id: 'Guide', destination: '/a/b' },
      'rendered artifact id must be 1-64 lowercase ASCII letters/digits with internal hyphens',
    ],
    [
      'a relative destination',
      { id: 'guide', destination: 'a/b' },
      'rendered destination must be an absolute Path',
    ],
    [
      'a root destination',
      { id: 'guide', destination: '/' },
      'rendered destination must be an absolute Path',
    ],
  ])('%s is refused with the crate text', (_, fields, message) => {
    expect(
      () => new RenderedArtifact({ family: 'instructions', content: section(), ...fields }),
    ).toThrow(message);
  });

  test('schema requirements need matching structured content', () => {
    const schema = new DocumentSchema('json', 'version', 1n);
    expect(
      () =>
        new RenderedArtifact({
          id: 'guide',
          family: 'hooks',
          destination: '/a/b',
          content: section(),
          schemaRequirements: [schema],
        }),
    ).toThrow('document schema requirements need matching structured content');
  });

  test('keeps matching schema requirements', () => {
    const artifact = new RenderedArtifact({
      id: 'hook',
      family: 'hooks',
      destination: '/a/config.json',
      content: new StructuredContent('json', new Selector([new Key('hooks')]), 1n),
      schemaRequirements: [new DocumentSchema('json', 'version', 1n)],
    });
    expect(artifact.schemaRequirements.map((schema) => [schema.key, schema.value])).toEqual([
      ['version', 1n],
    ]);
  });
});

describe('RenderedBundle', () => {
  const file = (id: string, destination: string): RenderedArtifact =>
    new RenderedArtifact({
      id,
      family: 'hooks',
      destination,
      content: new FileContent(new TextEncoder().encode('run\n'), 'executable'),
    });
  const guide = new RenderedArtifact({
    id: 'guide',
    family: 'instructions',
    destination: '/fixture/AGENTS.md',
    content: new SectionContent('guide', 'Hello'),
  });

  test('sorts artifacts by id and routing keys by name', () => {
    const bundle = new RenderedBundle(
      [file('launcher', '/fixture/launcher'), guide],
      [new Dependency('guide', 'launcher', 'stable-reference')],
      [{ artifactId: 'guide', kind: 'activation', message: 'Reload.', code: null }],
      { zeta: '1', alpha: '2' },
    );
    expect(bundle.artifacts.map((artifact) => artifact.id)).toEqual(['guide', 'launcher']);
    expect(Object.keys(bundle.routingContext)).toEqual(['alpha', 'zeta']);
    expect(bundle.notices).toEqual([
      { artifactId: 'guide', kind: 'activation', message: 'Reload.', code: null },
    ]);
    expect(bundle.supported).toBe(true);
  });

  test('empty has the empty digest and no artifacts', () => {
    const empty = RenderedBundle.empty();
    expect(empty.artifacts).toEqual([]);
    expect(empty.contentDigest).toBe(new RenderedBundle([]).contentDigest);
  });

  test('duplicate ids are refused with the crate text', () => {
    expect(() => new RenderedBundle([guide, guide])).toThrow(
      'rendered artifact ids must be unique',
    );
  });

  test('an unsupported notice makes the bundle unsupported', () => {
    expect(
      new RenderedBundle(
        [],
        [],
        [{ artifactId: 'guide', kind: 'unsupported', message: 'No.', code: 'no' }],
      ).supported,
    ).toBe(false);
  });

  test('a blank notice message is refused', () => {
    expect(
      () =>
        new RenderedBundle(
          [guide],
          [],
          [{ artifactId: 'guide', kind: 'activation', message: ' ', code: null }],
        ),
    ).toThrow('notice message must be nonblank');
  });
});
