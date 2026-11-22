import { describe, expect, test } from 'vitest';
import {
  Claim,
  DocumentProvenance,
  DocumentRequest,
  Key,
  Member,
  Placement,
  SectionBoundaries,
  SectionContent,
  Selector,
  StructuredContent,
  TimeValue,
  ValueError,
  editDocument,
  type Acquisition,
  type DocumentEdit,
  type DocumentFormat,
  type EditStatus,
  type OwnedSelection,
  type Position,
  type SelectionSnapshot,
  type SemanticValue,
} from 'flyrail';
import { fixture } from '../helpers/fixtures.js';
import { jsonValue, type Json } from '../helpers/json.js';

type Selection = Json;
interface Step {
  readonly action: 'set' | 'retire';
  readonly selector?: Selection;
  readonly expected?: string | null;
  readonly observed?: string | null;
  readonly status?: EditStatus;
  readonly statuses?: readonly EditStatus[];
  readonly text?: string;
  readonly marker?: string;
  readonly acquisition?: Acquisition;
  readonly value?: Json;
  readonly semantic_value?: readonly [string, string];
  readonly placement?: { readonly position: Position; readonly anchor?: Json };
  readonly requests?: readonly Step[];
}
interface EditCase {
  readonly name: string;
  readonly format: 'text' | DocumentFormat;
  readonly before?: string | null;
  readonly steps: readonly Step[];
}
interface InvalidCase {
  readonly name: string;
  readonly format: 'text' | DocumentFormat;
  readonly document: string;
  readonly selectors: readonly Selection[];
}

const invalidMessages: Readonly<Record<string, string>> = {
  'nested-custom-sections': 'nested or repeated section boundaries',
  'duplicate-json-key': 'duplicate JSON object key',
  'duplicate-jsonc-key': 'duplicate JSON object key',
  'duplicate-toml-key': 'Key "x" already exists. at line 2 col 0',
  'duplicate-array-identity': 'duplicate or ambiguous array member identities',
  'overlapping-paths': 'document claims overlap',
};

const document = fixture('edits.json') as
  { readonly cases: readonly EditCase[]; readonly invalid: readonly InvalidCase[] } | undefined;

const encode = (text: string | null | undefined): Uint8Array | null =>
  text === null || text === undefined ? null : new TextEncoder().encode(text);
const decode = (bytes: Uint8Array | null): string | null =>
  bytes === null ? null : new TextDecoder('utf-8', { ignoreBOM: true }).decode(bytes);

const semantic = (tagged: readonly [string, string]): SemanticValue => {
  if (tagged[0] !== 'time') {
    throw new Error(`unsupported semantic tag ${tagged[0]}`);
  }
  return TimeValue.parse(tagged[1]);
};

const boundaries = (selector: Selection): SectionBoundaries => {
  const record = selector as { readonly start: string; readonly end: string };
  return new SectionBoundaries(record.start, record.end);
};

const member = (record: Json): Member => {
  const item = record as {
    readonly identity?: Json;
    readonly semantic_identity?: readonly [string, string];
    readonly key?: readonly string[];
  };
  const identity =
    item.semantic_identity === undefined
      ? jsonValue(item.identity ?? null)
      : semantic(item.semantic_identity);
  return new Member(identity, item.key ?? []);
};

const structuredSelector = (selector: Selection): Selector =>
  new Selector(
    (selector as readonly Json[]).map((part) =>
      typeof part === 'string' ? new Key(part) : member(part),
    ),
  );

const request = (step: Step, format: 'text' | DocumentFormat): DocumentRequest => {
  const acquisition = step.acquisition ?? 'conflict';
  if (format === 'text') {
    const pair = boundaries(step.selector ?? null);
    if (step.action === 'retire') {
      return DocumentRequest.retire(new Claim('section', pair));
    }
    return DocumentRequest.set(
      new SectionContent(step.marker ?? 'fixture', step.text ?? '', pair),
      acquisition,
    );
  }
  const selector = structuredSelector(step.selector ?? null);
  if (step.action === 'retire') {
    return DocumentRequest.retire(new Claim('structured', selector));
  }
  const value =
    step.semantic_value === undefined
      ? jsonValue(step.value ?? null)
      : semantic(step.semantic_value);
  const placement =
    step.placement === undefined
      ? null
      : new Placement(
          step.placement.position,
          step.placement.anchor === undefined ? null : member(step.placement.anchor),
        );
  return DocumentRequest.set(
    new StructuredContent(format, selector, value, placement),
    acquisition,
  );
};

const hex = (bytes: Uint8Array | null): string | null =>
  bytes === null ? null : Buffer.from(bytes).toString('hex');

const snapshotSummary = (snapshot: SelectionSnapshot | null): unknown =>
  snapshot === null
    ? null
    : {
        value: snapshot.value.kind === 'bytes' ? hex(snapshot.value.bytes) : 'typed',
        syntax: hex(snapshot.syntax),
        ownedSyntax: hex(snapshot.ownedSyntax),
        layout: hex(snapshot.layout),
        prefix: hex(snapshot.position?.prefix ?? null),
      };

const selectionSummary = (owned: OwnedSelection): unknown => ({
  kind: owned.claim.kind,
  format: owned.format,
  disposition: owned.disposition,
  installed: snapshotSummary(owned.installed),
  baseline: snapshotSummary(owned.baseline),
  separator: hex(owned.separator),
});

const provenanceSummary = (provenance: DocumentProvenance): unknown => ({
  emptyDocument: hex(provenance.emptyDocument),
  schemaEmptyDocument: hex(provenance.schemaEmptyDocument),
  containers: provenance.containers.map((container) => hex(container.emptySyntax)),
  trailingSeparators: provenance.trailingSeparators,
  schemaFields: provenance.schemaFields.map(selectionSummary),
});

const checkOwnership = (
  index: number,
  result: DocumentEdit,
  previous: readonly OwnedSelection[],
  previousProvenance: DocumentProvenance,
  baselines: { readonly claim: Claim; readonly summary: unknown }[],
): void => {
  const where = `step ${String(index)}`;
  result.ownership.forEach((owned) => {
    if (owned.disposition !== 'taken-over') {
      expect(owned.baseline, `${where} only takeover owns a baseline`).toBeNull();
      return;
    }
    const summary = snapshotSummary(owned.baseline);
    const known = baselines.find((item) => item.claim.equals(owned.claim));
    if (known === undefined) {
      baselines.push({ claim: owned.claim, summary });
    } else {
      expect(summary, `${where} takeover baseline`).toEqual(known.summary);
    }
  });
  if (!result.applicable) {
    expect(result.ownership.map(selectionSummary), `${where} ownership`).toEqual(
      previous.map(selectionSummary),
    );
    expect(provenanceSummary(result.provenance), `${where} provenance`).toEqual(
      provenanceSummary(previousProvenance),
    );
  }
};

const replay = (testCase: EditCase): void => {
  let current = encode(testCase.before);
  let ownership: readonly OwnedSelection[] = [];
  let provenance = new DocumentProvenance();
  const baselines: { readonly claim: Claim; readonly summary: unknown }[] = [];
  testCase.steps.forEach((step, index) => {
    const observed = step.observed === undefined ? current : encode(step.observed);
    const requests = (step.requests ?? [step]).map((item) => request(item, testCase.format));
    const result = editDocument(observed, requests, ownership, provenance);
    expect(decode(result.after), `step ${String(index)} after`).toBe(step.expected ?? null);
    expect(
      result.observations.map((observation) => observation.status),
      `step ${String(index)} statuses`,
    ).toEqual(step.statuses ?? [step.status]);
    checkOwnership(index, result, ownership, provenance, baselines);
    current = result.after;
    ownership = result.ownership;
    provenance = result.provenance;
  });
};

describe.skipIf(document === undefined)('edits.json', () => {
  const cases = document?.cases ?? [];
  const invalid = document?.invalid ?? [];

  test('has cases', () => {
    expect(cases.length).toBeGreaterThan(0);
    expect(invalid.length).toBeGreaterThan(0);
  });

  test.each(cases.map((testCase) => [testCase.name, testCase] as const))(
    '%s replays with the shared bytes and statuses',
    (_, testCase) => {
      replay(testCase);
    },
  );

  test.each(invalid.map((testCase) => [testCase.name, testCase] as const))(
    '%s is rejected with the shared message',
    (name, testCase) => {
      const message = invalidMessages[name];
      expect(message, `invalid case ${name} has an expected message`).toBeDefined();
      const requests = testCase.selectors.map((selector) =>
        testCase.format === 'text'
          ? DocumentRequest.set(new SectionContent('fixture', 'body', boundaries(selector)))
          : DocumentRequest.set(
              new StructuredContent(testCase.format, structuredSelector(selector), 1n),
            ),
      );
      expect(() => editDocument(encode(testCase.document), requests)).toThrow(
        new ValueError(message ?? ''),
      );
    },
  );
});
