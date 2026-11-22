import { describe, expect, test } from 'vitest';
import {
  Bundle,
  FileContent,
  InstructionArtifact,
  McpArtifact,
  RenderContext,
  SectionContent,
  StructuredContent,
  Target,
  fileEntry,
  directoryEntry,
  renderMany,
  render,
  translateInstructionArtifact,
  translateMcpArtifact,
  valuesEqual,
  type Agent,
  type ArtifactTranslation,
  type Key,
  type Platform,
  type Surface,
  type TargetScope,
} from 'flyrail';
import { fixture } from '../helpers/fixtures.js';
import { jsonValue, type Json } from '../helpers/json.js';

interface TranslationCase {
  readonly name: string;
  readonly agent: Agent;
  readonly surface: Surface;
  readonly scope: TargetScope;
  readonly execution_root?: string;
  readonly mcp_path?: string;
  readonly artifact: { readonly id: string; readonly kind: 'instruction' | 'mcp' } & Record<
    string,
    Json
  >;
  readonly expected: {
    readonly destination?: string;
    readonly directory?: string;
    readonly suffix?: string;
    readonly format?: string;
    readonly key?: string;
    readonly value?: { readonly [key: string]: Json };
    readonly text?: string;
    readonly unsupported?: string;
    readonly cwd_from_root?: string;
  };
}

interface CombinedCase {
  readonly name: string;
  readonly artifacts: readonly Json[];
  readonly contexts: readonly { readonly agent: Agent }[];
  readonly expected: { readonly unsupported?: string };
}

interface TranslationDocument {
  readonly cases: readonly TranslationCase[];
  readonly asset_manifest: { readonly artifacts: readonly Json[] } & Record<string, Json>;
  readonly combined_cases: readonly CombinedCase[];
}

const document = fixture('translations.json') as TranslationDocument | undefined;

const root = '/fixture/translation';
const platforms: readonly Platform[] = ['macos', 'linux', 'windows'];
const unsupportedMessages: Readonly<Record<string, string>> = {
  'instruction-destination': 'Cursor user rules have no verified filesystem preset.',
  'portable-auth-env-unverified': 'Copilot CLI bearer environment syntax is unverified.',
  'literal-interpolation': 'env contains host interpolation syntax; use native content.',
};

const manifestBundle = (
  manifest: Record<string, Json>,
  entries: Parameters<typeof Bundle.fromMemory>[1] = [],
): Bundle =>
  Bundle.fromMemory(
    new TextEncoder().encode(
      JSON.stringify({ schema_version: 2, id: 'translation-demo', version: 'same', ...manifest }),
    ),
    entries,
  );

const artifactOf = (source: Json): InstructionArtifact | McpArtifact => {
  const artifact = manifestBundle({ artifacts: [source] }).artifacts[0];
  if (artifact instanceof InstructionArtifact || artifact instanceof McpArtifact) {
    return artifact;
  }
  throw new Error('fixture artifact is neither an instruction nor an MCP server');
};

const contextOf = (
  agent: Agent,
  platform: Platform,
  scope: TargetScope,
  options: ConstructorParameters<typeof RenderContext>[2],
): RenderContext =>
  new RenderContext(
    scope === 'user' ? Target.user(agent, root) : Target.project(agent, root),
    platform,
    options,
  );

const translate = (
  testCase: TranslationCase,
  platform: Platform,
): { readonly translation: ArtifactTranslation; readonly instruction: boolean } => {
  const context = contextOf(testCase.agent, platform, testCase.scope, {
    surface: testCase.surface,
    ...(testCase.execution_root === undefined
      ? {}
      : { executionRoot: `${root}/${testCase.execution_root}` }),
    ...(testCase.mcp_path === undefined ? {} : { mcpPath: `${root}/${testCase.mcp_path}` }),
  });
  const artifact = artifactOf(testCase.artifact);
  return artifact instanceof InstructionArtifact
    ? {
        translation: translateInstructionArtifact('translation-demo', artifact, context),
        instruction: true,
      }
    : { translation: translateMcpArtifact(artifact, context, {}), instruction: false };
};

describe.skipIf(document === undefined)('translations.json cases', () => {
  const cases = document?.cases ?? [];

  test('covers instruction and mcp artifacts', () => {
    const kinds = new Set(cases.map((testCase) => testCase.artifact.kind));
    expect([...kinds].sort()).toEqual(['instruction', 'mcp']);
  });

  test.each(
    platforms.flatMap((platform) =>
      cases.map((testCase) => [`${testCase.name} on ${platform}`, testCase, platform] as const),
    ),
  )('%s translates to the shared content', (_, testCase, platform) => {
    const { translation } = translate(testCase, platform);
    const { expected } = testCase;
    if (expected.unsupported !== undefined) {
      expect(translation.supported).toBe(false);
      expect(translation.artifact).toBeNull();
      expect(translation.notices).toEqual([
        {
          artifactId: testCase.artifact.id,
          kind: 'unsupported',
          message: unsupportedMessages[expected.unsupported],
          code: expected.unsupported,
        },
      ]);
      return;
    }
    const artifact = translation.artifact;
    expect(translation.supported).toBe(true);
    expect(artifact?.schemaRequirements).toEqual([]);
    if (expected.destination !== undefined) {
      expect(artifact?.destination).toBe(`${root}/${expected.destination}`);
    }
    if (expected.directory !== undefined) {
      const destination = artifact?.destination as string;
      const name = destination.slice(destination.lastIndexOf('/') + 1);
      expect(destination).toBe(`${root}/${expected.directory}/${name}`);
      expect(name.endsWith(expected.suffix as string)).toBe(true);
      expect(name.length).toBeGreaterThan((expected.suffix as string).length);
    }
    const content = artifact?.content;
    if (content instanceof StructuredContent) {
      const value = {
        ...expected.value,
        ...(expected.cwd_from_root === undefined
          ? {}
          : { cwd: `${root}/${expected.cwd_from_root}` }),
      } as Json;
      expect(content.format).toBe(expected.format);
      expect(valuesEqual(content.value, jsonValue(value))).toBe(true);
      expect(content.selector.parts.map((part) => (part as Key).name)).toEqual([
        expected.key,
        'tools',
      ]);
    } else if (content instanceof SectionContent) {
      expect(content.text).toBe(expected.text);
    } else {
      expect(content).toBeInstanceOf(FileContent);
      expect(new TextDecoder().decode((content as FileContent).data)).toBe(expected.text);
    }
  });
});

describe.skipIf(document === undefined)('translations.json asset_manifest', () => {
  test('substitutes the pinned revision into the translated MCP server', () => {
    const manifest = document?.asset_manifest as TranslationDocument['asset_manifest'];
    const bundle = manifestBundle(manifest, [
      fileEntry('runtime/server.js', new TextEncoder().encode('console.log("MCP");')),
      directoryEntry('runtime/data'),
    ]);
    const context = contextOf('codex', 'linux', 'project', { assetRoot: `${root}/assets` });
    const rendered = render(bundle, context);
    expect(rendered.supported).toBe(true);
    expect(rendered.artifacts).toHaveLength(2);
    expect(rendered.dependencies).toHaveLength(1);
    const [runtime, tools] = rendered.artifacts;
    const destination = runtime?.destination as string;
    expect(destination.slice(0, destination.lastIndexOf('/'))).toBe(
      `${root}/assets/support-demo/runtime`,
    );
    expect(destination.slice(destination.lastIndexOf('/') + 1)).toHaveLength(64);
    expect(
      valuesEqual(
        (tools?.content as StructuredContent).value,
        jsonValue({
          command: 'node',
          args: [`${destination}/server.js`],
          env: {},
          cwd: `${destination}/data`,
        }),
      ),
    ).toBe(true);
  });
});

describe.skipIf(document === undefined)('translations.json combined_cases', () => {
  const cases = document?.combined_cases ?? [];

  test('has cases', () => {
    expect(cases.length).toBeGreaterThan(0);
  });

  test.each(cases.map((testCase) => [testCase.name, testCase] as const))(
    '%s is independent of context order',
    (_, testCase) => {
      const bundle = manifestBundle({ id: 'combined', artifacts: testCase.artifacts });
      const contexts = testCase.contexts.map((entry) =>
        contextOf(entry.agent, 'linux', 'project', {}),
      );
      const forward = renderMany(bundle, contexts);
      const reverse = renderMany(bundle, [...contexts].reverse());
      expect(forward.contentDigest).toBe(reverse.contentDigest);
      expect(forward.notices).toEqual(reverse.notices);
      expect(forward.routingContext).toEqual(reverse.routingContext);
      const unsupported = forward.notices
        .filter((notice) => notice.kind === 'unsupported')
        .map((notice) => notice.code);
      if (testCase.expected.unsupported !== undefined) {
        expect(forward.supported).toBe(false);
        expect(forward.artifacts).toEqual([]);
        expect(unsupported).toEqual([testCase.expected.unsupported]);
      } else {
        expect(forward.supported).toBe(true);
        expect(forward.artifacts).toHaveLength(2);
        expect(forward.dependencies).toEqual([]);
        expect(Object.keys(forward.routingContext)).toHaveLength(2);
      }
    },
  );
});
