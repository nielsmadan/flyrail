import { describe, expect, test } from 'vitest';
import {
  ArrayPosition,
  Claim,
  CreatedContainer,
  DocumentProvenance,
  DocumentRequest,
  FileContent,
  Key,
  Member,
  ObjectValue,
  OwnedSelection,
  Placement,
  SectionContent,
  SelectionSnapshot,
  Selector,
  StructuredContent,
  TreeContent,
  ValueError,
  contentClaim,
  editDocument,
  valuesEqual,
} from 'flyrail';

const encode = (text: string): Uint8Array => new TextEncoder().encode(text);
const decode = (bytes: Uint8Array | null): string | null =>
  bytes === null ? null : new TextDecoder().decode(bytes);
const setting = (value: bigint): StructuredContent =>
  new StructuredContent('json', new Selector([new Key('limit')]), value);

const jsonItem = (name: string): ObjectValue => new ObjectValue([['name', name]]);

describe('contentClaim', () => {
  test('a section content claims its boundaries', () => {
    const claim = contentClaim(new SectionContent('guide', 'x'));
    expect(claim.kind).toBe('section');
    expect(claim.equals(contentClaim(new SectionContent('guide', 'other')))).toBe(true);
  });

  test('a structured content claims its selector', () => {
    const claim = contentClaim(setting(1n));
    expect(claim.kind).toBe('structured');
    expect(claim.equals(new Claim('structured', new Selector([new Key('limit')])))).toBe(true);
  });

  test('file content cannot be edited as a document', () => {
    expect(() => contentClaim(new FileContent(encode('x'), 'private'))).toThrow(
      new ValueError('document content must be a section or structured selection'),
    );
    expect(() => contentClaim(new TreeContent([]))).toThrow(
      new ValueError('document content must be a section or structured selection'),
    );
  });
});

describe('editDocument', () => {
  test('a section request on an absent document creates it', () => {
    const edit = editDocument(null, [DocumentRequest.set(new SectionContent('guide', 'x'))]);
    expect(decode(edit.after)).toBe(
      '<!-- flyrail:guide:start -->\nx\n<!-- flyrail:guide:end -->\n',
    );
    expect(edit.before).toBeNull();
    expect(edit.observations.map((observation) => observation.status)).toEqual(['created']);
    expect(edit.applicable).toBe(true);
    expect(edit.ownership.map((owned) => owned.disposition)).toEqual(['created']);
  });

  test('feeding ownership and provenance back into a retirement deletes the document', () => {
    const installed = editDocument(null, [DocumentRequest.set(new SectionContent('guide', 'x'))]);
    const retired = editDocument(
      installed.after,
      installed.ownership.map((owned) => DocumentRequest.retire(owned.claim)),
      installed.ownership,
      installed.provenance,
    );
    expect(retired.after).toBeNull();
    expect(retired.observations.map((observation) => observation.status)).toEqual(['removed']);
    expect(retired.ownership).toEqual([]);
  });

  test('a foreign edit of a structured value reports modified on the next apply', () => {
    const installed = editDocument(encode('{}\n'), [DocumentRequest.set(setting(1n))]);
    expect(decode(installed.after)).toBe('{"limit":1}\n');
    const foreign = encode((decode(installed.after) ?? '').replace('1', '2'));
    const reapplied = editDocument(
      foreign,
      [DocumentRequest.set(setting(1n))],
      installed.ownership,
      installed.provenance,
    );
    expect(reapplied.observations.map((observation) => observation.status)).toEqual(['modified']);
    expect(reapplied.applicable).toBe(false);
    expect(decode(reapplied.after)).toBe(decode(foreign));
  });

  test('an unchanged document reports current', () => {
    const installed = editDocument(encode('{}\n'), [DocumentRequest.set(setting(1n))]);
    const again = editDocument(
      installed.after,
      [DocumentRequest.set(setting(1n))],
      installed.ownership,
      installed.provenance,
    );
    expect(again.observations.map((observation) => observation.status)).toEqual(['current']);
  });

  test('an installed selection carries its snapshot, format and separator', () => {
    const installed = editDocument(encode('{}\n'), [DocumentRequest.set(setting(7n))]);
    const [owned] = installed.ownership;
    expect(owned?.installed.value.kind).toBe('typed');
    const value = owned?.installed.value;
    expect(value?.kind === 'typed' && valuesEqual(value.value, 7n)).toBe(true);
    expect(owned?.format).toBe('json');
    expect(owned?.separator).toBeNull();
  });

  test('a conflict observation carries the crate message', () => {
    const content = new StructuredContent(
      'json',
      new Selector([new Key('items'), new Member('b', ['name'])]),
      jsonItem('b'),
      new Placement('after', new Member('missing', ['name'])),
    );
    const edit = editDocument(encode('{"items":[{"name":"a"}]}\n'), [DocumentRequest.set(content)]);
    const [observation] = edit.observations;
    expect(observation?.status).toBe('conflict');
    expect(observation?.message).toBe('array placement anchor is missing');
  });

  test('a modified observation carries the live snapshot', () => {
    const installed = editDocument(encode('{}\n'), [DocumentRequest.set(setting(1n))]);
    const foreign = encode((decode(installed.after) ?? '').replace('1', '2'));
    const reapplied = editDocument(
      foreign,
      [DocumentRequest.set(setting(1n))],
      installed.ownership,
      installed.provenance,
    );
    const live = reapplied.observations[0]?.current?.value;
    expect(live?.kind === 'typed' && valuesEqual(live.value, 2n)).toBe(true);
  });

  test('byte arguments must be Uint8Array', () => {
    expect(() => editDocument('abc' as never, [])).toThrow(ValueError);
    expect(() => new ArrayPosition(null, null, 'abc' as never)).toThrow(ValueError);
  });

  test('duplicate JSON keys throw the crate message', () => {
    expect(() =>
      editDocument(encode('{"limit":1,"limit":1}'), [DocumentRequest.set(setting(1n))]),
    ).toThrow(new ValueError('duplicate JSON object key'));
  });

  test('a request whose claim does not match its content throws', () => {
    const claim = new Claim('structured', new Selector([new Key('other')]));
    expect(() => new DocumentRequest(claim, setting(1n))).toThrow(
      new ValueError('requested content must address its physical claim'),
    );
  });

  test('a retirement cannot acquire ownership', () => {
    const claim = contentClaim(setting(1n));
    expect(() => new DocumentRequest(claim, null, 'takeover')).toThrow(
      new ValueError('retirement does not acquire ownership'),
    );
  });

  test('inputs of the wrong class throw ValueError', () => {
    expect(() => editDocument(null, [{} as DocumentRequest])).toThrow(ValueError);
  });

  test('byte accessors return copies', () => {
    const installed = editDocument(encode('{}\n'), [DocumentRequest.set(setting(1n))]);
    const snapshot = installed.ownership[0]?.installed;
    const syntax = snapshot?.syntax ?? new Uint8Array();
    syntax.fill(0);
    expect(decode(snapshot?.syntax ?? null)).toBe('1');
  });
});

describe('editing models', () => {
  const member = (name: string): Member => new Member(name, ['name']);

  test('an array position keeps its anchors and prefix', () => {
    const position = new ArrayPosition(member('a'), member('b'), encode(','));
    expect(position.previous?.identity).toBe('a');
    expect(position.following?.identity).toBe('b');
    expect(decode(position.prefix)).toBe(',');
  });

  test('array position anchors must be distinct', () => {
    expect(() => new ArrayPosition(member('a'), member('a'), encode(''))).toThrow(
      new ValueError('array position anchors must be distinct'),
    );
  });

  test('a snapshot round trips bytes and typed values', () => {
    const bytes = new SelectionSnapshot({
      value: { kind: 'bytes', bytes: encode('text') },
      syntax: encode('s'),
      ownedSyntax: encode('o'),
      layout: new Uint8Array(),
    });
    expect(decode(bytes.value.kind === 'bytes' ? bytes.value.bytes : null)).toBe('text');
    expect(bytes.position).toBeNull();
    const typed = new SelectionSnapshot({
      value: { kind: 'typed', value: 3n },
      syntax: encode('3'),
      ownedSyntax: encode('3'),
      layout: new Uint8Array(),
    });
    expect(typed.value.kind === 'typed' && typed.value.value).toBe(3n);
  });

  test('only a takeover owns a baseline', () => {
    const snapshot = new SelectionSnapshot({
      value: { kind: 'bytes', bytes: encode('a') },
      syntax: encode('a'),
      ownedSyntax: encode('a'),
      layout: new Uint8Array(),
    });
    expect(
      () =>
        new OwnedSelection({
          claim: contentClaim(new SectionContent('guide', 'x')),
          installed: snapshot,
          disposition: 'created',
          baseline: snapshot,
        }),
    ).toThrow(new ValueError('only takeover ownership has a baseline'));
  });

  test('provenance validates separator digests and keeps containers', () => {
    expect(() => new DocumentProvenance({ trailingSeparators: ['nope'] })).toThrow(
      new ValueError('separator evidence must be SHA-256 digests'),
    );
    const container = new CreatedContainer(new Selector([new Key('mcp')]), encode('{}'));
    const provenance = new DocumentProvenance({
      containers: [container],
      emptyDocument: encode(''),
    });
    expect(provenance.containers).toHaveLength(1);
    expect(decode(provenance.containers[0]?.emptySyntax ?? null)).toBe('{}');
    expect(decode(provenance.emptyDocument)).toBe('');
    expect(provenance.schemaEmptyDocument).toBeNull();
  });
});
