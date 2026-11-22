import { createHash } from 'node:crypto';
import { describe, expect, test } from 'vitest';
import {
  Bundle,
  BundleIdentity,
  Claim,
  Command,
  Dependency,
  DocumentSchema,
  FileContent,
  HookArtifact,
  HookRuntime,
  McpArtifact,
  HttpTransport,
  EnvRef,
  RenderContext,
  RenderedArtifact,
  RenderedBundle,
  SectionContent,
  StructuredContent,
  Target,
  capabilities,
  render,
  translateMcpArtifact,
  type Agent,
  type Platform,
  type Surface,
  type TargetScope,
  type Content,
  type DependencyMode,
  type DocumentFormat,
  type Family,
  DateTimeValue,
  DateValue,
  Key,
  Member,
  ObjectValue,
  ResourceAuthority,
  SectionBoundaries,
  Selector,
  TimeValue,
  WHOLE,
  directoryEntry,
  encodeValue,
  fileEntry,
  transactionPath,
  type ClaimSelector,
  type Ownership,
  type SemanticValue,
  valuesEqual,
} from 'flyrail';
import { fixture } from '../helpers/fixtures.js';
import { jsonValue, type Json } from '../helpers/json.js';
import { fromHex } from '../helpers/bytes.js';

interface SemanticCase {
  readonly record: readonly unknown[];
  readonly encoding_hex: string;
  readonly sha256: string;
}

interface ConfigurationCase {
  readonly name: string;
  readonly manifest: unknown;
  readonly entries: readonly { readonly path: string; readonly data_hex: string | null }[];
  readonly content_digest: string;
}

interface ConfigurationDocument {
  readonly semantic_values: readonly SemanticCase[];
  readonly cases: readonly ConfigurationCase[];
}

const document = fixture('configurations.json') as ConfigurationDocument | undefined;

const hexFloat = /^(-?)0x([01])\.([0-9a-f]*)p([+-]\d+)$/;

function parseHexFloat(text: string): number {
  const match = hexFloat.exec(text);
  if (match === null) {
    throw new Error(`unrecognised float ${text}`);
  }
  const [, sign = '', whole = '0', fraction = '', exponent = '0'] = match;
  const mantissa =
    Number.parseInt(whole, 10) +
    (fraction === '' ? 0 : Number(BigInt(`0x${fraction}`)) / 16 ** fraction.length);
  const magnitude = mantissa * 2 ** Number.parseInt(exponent, 10);
  return sign === '-' ? -magnitude : magnitude;
}

function fromRecord(record: readonly unknown[]): SemanticValue {
  const [kind, payload] = record as [string, unknown];
  switch (kind) {
    case 'null':
      return null;
    case 'boolean':
      return payload as boolean;
    case 'integer':
      return BigInt(payload as string);
    case 'float':
      return parseHexFloat(payload as string);
    case 'string':
      return payload as string;
    case 'date':
      return DateValue.parse(payload as string);
    case 'time':
      return TimeValue.parse(payload as string);
    case 'datetime':
      return DateTimeValue.parse(payload as string);
    case 'array':
      return (payload as readonly (readonly unknown[])[]).map(fromRecord);
    case 'object':
      return new ObjectValue(
        (payload as readonly (readonly [string, readonly unknown[]])[]).map(
          ([key, member]) => [key, fromRecord(member)] as const,
        ),
      );
    default:
      throw new Error(`unrecognised record kind ${kind}`);
  }
}

describe.skipIf(document === undefined)('configurations.json semantic_values', () => {
  const cases = document?.semantic_values ?? [];

  test('has cases', () => {
    expect(cases.length).toBeGreaterThan(0);
  });

  test.each(cases.map((testCase) => [JSON.stringify(testCase.record), testCase] as const))(
    '%s encodes to the shared bytes',
    (_, testCase) => {
      const encoded = encodeValue(fromRecord(testCase.record));
      expect(Buffer.from(encoded).toString('hex')).toBe(testCase.encoding_hex);
      expect(createHash('sha256').update(encoded).digest('hex')).toBe(testCase.sha256);
    },
  );
});

describe.skipIf(document === undefined)('configurations.json cases', () => {
  const cases = document?.cases ?? [];

  test('has cases', () => {
    expect(cases.length).toBeGreaterThan(0);
  });

  test.each(cases.map((testCase) => [testCase.name, testCase] as const))(
    '%s yields the shared digest',
    (_, testCase) => {
      const bundle = Bundle.fromMemory(
        new TextEncoder().encode(JSON.stringify(testCase.manifest)),
        testCase.entries.map((entry) =>
          entry.data_hex === null
            ? directoryEntry(entry.path)
            : fileEntry(entry.path, fromHex(entry.data_hex)),
        ),
      );
      expect(bundle.schemaVersion).toBe(2);
      expect(bundle.contentDigest).toBe(testCase.content_digest);
      const rebuilt = Bundle.fromArtifacts(
        bundle.identity,
        bundle.artifacts,
        bundle.assets,
        bundle.dependencies,
      );
      expect(rebuilt.contentDigest).toBe(testCase.content_digest);
    },
  );
});

interface ClaimVector {
  readonly name: string;
  readonly destination: string;
  readonly kind: Ownership;
  readonly selector: unknown;
  readonly identity: string;
}

interface AuthorityVector {
  readonly name: string;
  readonly destination: string;
  readonly transaction_id: string;
  readonly transaction_path: string;
}

interface ClaimsDocument {
  readonly claims: readonly ClaimVector[];
  readonly claim_overlaps: readonly {
    readonly first: string;
    readonly second: string;
    readonly overlaps: boolean;
  }[];
  readonly authorities: readonly AuthorityVector[];
}

const claimsDocument = document as unknown as ClaimsDocument | undefined;

function claimSelector(selector: unknown): ClaimSelector {
  if (selector === null) {
    return WHOLE;
  }
  if (typeof selector === 'string') {
    return { kind: 'subtree', path: selector };
  }
  const items = selector as readonly unknown[];
  if (items.length === 2 && items.every((item) => typeof item === 'string')) {
    return new SectionBoundaries(items[0] as string, items[1] as string);
  }
  return new Selector(
    items.map((item) => {
      const [tag, ...rest] = item as readonly unknown[];
      if (tag === 'key') {
        return new Key(rest[0] as string);
      }
      return new Member(fromRecord(rest[1] as readonly unknown[]), rest[0] as readonly string[]);
    }),
  );
}

const claimOf = (vector: ClaimVector): Claim =>
  new Claim(vector.kind, claimSelector(vector.selector));

describe.skipIf(claimsDocument === undefined)('configurations.json claims', () => {
  const claims = claimsDocument?.claims ?? [];
  const byName = new Map(claims.map((vector) => [vector.name, claimOf(vector)] as const));

  test('has vectors', () => {
    expect(claims.length).toBeGreaterThan(0);
  });

  test.each(claims.map((vector) => [vector.name, vector] as const))(
    '%s has the shared identity',
    async (_, vector) => {
      const authority = await ResourceAuthority.create(vector.destination);
      expect(authority.destination).toBe(vector.destination);
      expect(claimOf(vector).identity(authority)).toBe(vector.identity);
    },
  );

  test.each(
    (claimsDocument?.claim_overlaps ?? []).map(
      (vector) => [`${vector.first} / ${vector.second}`, vector] as const,
    ),
  )('%s overlaps as the shared vector says', (_, vector) => {
    const first = byName.get(vector.first) as Claim;
    const second = byName.get(vector.second) as Claim;
    expect(first.overlaps(second)).toBe(vector.overlaps);
    expect(second.overlaps(first)).toBe(vector.overlaps);
  });
});

describe.skipIf(claimsDocument === undefined)('configurations.json authorities', () => {
  test.each((claimsDocument?.authorities ?? []).map((vector) => [vector.name, vector] as const))(
    '%s has the shared transaction path',
    (_, vector) => {
      expect(transactionPath(vector.destination, vector.transaction_id)).toBe(
        vector.transaction_path,
      );
    },
  );
});

interface RenderingVector {
  readonly name: string;
  readonly artifacts: readonly {
    readonly id: string;
    readonly family: Family;
    readonly destination: string;
    readonly content: {
      readonly kind: string;
      readonly marker?: string;
      readonly text?: string;
      readonly data_hex?: string;
      readonly format?: DocumentFormat;
      readonly selector?: readonly (
        string | { readonly identity: Json; readonly key: readonly string[] }
      )[];
      readonly value?: Json;
    };
    readonly schema_requirements?: readonly {
      readonly format: DocumentFormat;
      readonly key: string;
      readonly value: Json;
    }[];
  }[];
  readonly dependencies: readonly {
    readonly dependent: string;
    readonly required: string;
    readonly mode?: DependencyMode;
  }[];
  readonly content_digest: string;
}

function renderedContent(record: RenderingVector['artifacts'][number]['content']): Content {
  switch (record.kind) {
    case 'section':
      return new SectionContent(record.marker as string, record.text as string);
    case 'file':
      return new FileContent(fromHex(record.data_hex as string), 'private');
    case 'structured':
      return new StructuredContent(
        record.format as DocumentFormat,
        new Selector(
          (record.selector ?? []).map((part) =>
            typeof part === 'string'
              ? new Key(part)
              : new Member(jsonValue(part.identity), part.key),
          ),
        ),
        jsonValue(record.value as Json),
      );
    default:
      throw new Error(`unrecognised content kind ${record.kind}`);
  }
}

const renderings = (
  document as unknown as { readonly renderings?: readonly RenderingVector[] } | undefined
)?.renderings;

describe.skipIf(renderings === undefined)('configurations.json renderings', () => {
  test('has vectors', () => {
    expect(renderings?.length).toBeGreaterThan(0);
  });

  test.each((renderings ?? []).map((vector) => [vector.name, vector] as const))(
    '%s reproduces its content digest',
    (_, vector) => {
      const rendered = new RenderedBundle(
        vector.artifacts.map(
          (artifact) =>
            new RenderedArtifact({
              id: artifact.id,
              family: artifact.family,
              destination: artifact.destination,
              content: renderedContent(artifact.content),
              schemaRequirements: (artifact.schema_requirements ?? []).map(
                (schema) =>
                  new DocumentSchema(
                    schema.format,
                    schema.key,
                    jsonValue(schema.value) as ConstructorParameters<typeof DocumentSchema>[2],
                  ),
              ),
            }),
        ),
        vector.dependencies.map(
          (edge) => new Dependency(edge.dependent, edge.required, edge.mode ?? 'revision'),
        ),
      );
      expect(rendered.contentDigest).toBe(vector.content_digest);
    },
  );
});

interface CapabilityVector {
  readonly name: string;
  readonly context: {
    readonly agent: Agent;
    readonly scope: TargetScope;
    readonly surface: Surface;
    readonly platform: Platform;
  };
  readonly request: {
    readonly id: string;
    readonly kind: 'mcp' | 'hook';
    readonly name?: string;
    readonly transport?: {
      readonly kind: 'stdio' | 'http';
      readonly command?: {
        readonly argv: readonly string[];
        readonly env?: Readonly<Record<string, string>>;
      };
      readonly url?: string;
      readonly bearer_token_env?: string;
    };
    readonly event?: 'before-tool';
    readonly command?: { readonly argv: readonly string[] };
    readonly outcomes?: readonly ('ask' | 'continue')[];
  };
  readonly expected: {
    readonly supported: boolean;
    readonly destination?: string;
    readonly format?: DocumentFormat;
    readonly selector?: readonly string[];
    readonly value?: Json;
    readonly reason?: string;
    readonly registration_fields?: Readonly<Record<string, Json>>;
    readonly notices?: readonly { readonly kind: string; readonly requirement?: string }[];
  };
}

const capabilityVectors = (
  document as unknown as { readonly capabilities?: readonly CapabilityVector[] } | undefined
)?.capabilities;

const capabilityRoot = '/fixture/capabilities';

const implementedReasons: Readonly<Record<string, string>> = {
  'generic-preToolUse-ask-is-ignored': 'hook-outcome',
};

const implementedValue = (vector: CapabilityVector): Json => {
  const value = vector.expected.value as { readonly [key: string]: Json };
  return vector.name === 'copilot-cli-stdio'
    ? Object.fromEntries(Object.entries(value).filter(([key]) => key !== 'type'))
    : value;
};

function capabilityContext(vector: CapabilityVector): RenderContext {
  const { agent, scope, surface, platform } = vector.context;
  const target =
    scope === 'user' ? Target.user(agent, capabilityRoot) : Target.project(agent, capabilityRoot);
  return new RenderContext(target, platform, {
    surface,
    hookRuntime: new HookRuntime({ node: '/opt/node/bin/node', shell: 'posix' }),
  });
}

function capabilityTranslation(vector: CapabilityVector) {
  const { request } = vector;
  const transport = request.transport;
  const artifact = new McpArtifact(
    request.id,
    request.name ?? request.id,
    transport?.kind === 'http'
      ? new HttpTransport(
          transport.url as string,
          transport.bearer_token_env === undefined ? null : new EnvRef(transport.bearer_token_env),
        )
      : new Command(transport?.command?.argv ?? [], transport?.command?.env ?? {}),
  );
  return translateMcpArtifact(artifact, capabilityContext(vector), {});
}

function selectorNames(content: StructuredContent): readonly string[] {
  return content.selector.parts.map((part) => (part as Key).name);
}

describe.skipIf(capabilityVectors === undefined)('configurations.json capabilities', () => {
  test('has vectors', () => {
    expect(capabilityVectors?.length).toBeGreaterThan(0);
  });

  test.each(
    (capabilityVectors ?? [])
      .filter((vector) => vector.request.kind === 'mcp')
      .map((vector) => [vector.name, vector] as const),
  )('%s translates as the shared vector expects', (_, vector) => {
    const translation = capabilityTranslation(vector);
    const { expected } = vector;
    expect(translation.supported).toBe(expected.supported);
    if (!expected.supported) {
      expect(translation.artifact).toBeNull();
      expect(translation.notices.map((notice) => notice.code)).toContain(expected.reason);
      return;
    }
    const content = translation.artifact?.content as StructuredContent;
    const context = capabilityContext(vector);
    if (expected.destination !== undefined) {
      expect(translation.artifact?.destination).toBe(`${context.root}/${expected.destination}`);
      expect(content.format).toBe(expected.format);
      expect(selectorNames(content)).toEqual(expected.selector);
      expect(valuesEqual(content.value, jsonValue(implementedValue(vector)))).toBe(true);
    }
    if (expected.registration_fields !== undefined) {
      expect(valuesEqual(content.value, jsonValue(expected.registration_fields))).toBe(true);
    }
    const expectedNotices = expected.notices ?? [];
    const prerequisites = translation.notices.filter((notice) => notice.kind === 'prerequisite');
    expect(prerequisites).toHaveLength(expectedNotices.length);
    expectedNotices.forEach((notice, index) => {
      expect(prerequisites[index]?.message).toContain(notice.requirement ?? '');
    });
  });

  test.each(
    (capabilityVectors ?? [])
      .filter((vector) => vector.request.kind === 'hook')
      .map((vector) => [vector.name, vector] as const),
  )('%s renders as the shared vector expects', (_, vector) => {
    const { request, expected } = vector;
    const bundle = Bundle.fromArtifacts(new BundleIdentity('capability', '1'), [
      new HookArtifact({
        id: request.id,
        event: request.event as 'before-tool',
        command: new Command(request.command?.argv ?? []),
        timeoutMs: 1000,
        outcomes: [...(request.outcomes ?? [])],
      }),
    ]);
    const rendered = render(bundle, capabilityContext(vector));
    expect(rendered.supported).toBe(expected.supported);
    expect(rendered.artifacts.length > 0).toBe(expected.supported);
    expect(rendered.notices.map((notice) => notice.code)).toContain(
      implementedReasons[expected.reason as string] ?? expected.reason,
    );
  });

  test.each((capabilityVectors ?? []).map((vector) => [vector.name, vector] as const))(
    '%s reports one capability per family',
    (_, vector) => {
      const reported = capabilities(capabilityContext(vector));
      expect(reported.map((item) => item.family)).toEqual([
        'skills',
        'instructions',
        'mcp',
        'hooks',
      ]);
      const required = (vector.expected.notices ?? []).map((notice) => notice.requirement);
      const mcp = reported.find((item) => item.family === 'mcp');
      expect(mcp?.prerequisites.map((item) => item.split('@')[0])).toEqual(
        required.filter((item) => item !== undefined),
      );
    },
  );
});
