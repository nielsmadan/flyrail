import { describe, expect, test } from 'vitest';
import {
  DocumentSchema,
  FileContent,
  Key,
  Member,
  Placement,
  SectionBoundaries,
  SectionContent,
  Selector,
  StructuredContent,
  TreeContent,
  ValueError,
  directoryEntry,
  fileEntry,
} from 'flyrail';

describe('content', () => {
  test('structured content keeps a bigint value', () => {
    const content = new StructuredContent('json', new Selector([new Key('a')]), 1n);
    expect(content.value).toBe(1n);
  });

  test('a section marker yields the Rust boundaries', () => {
    const boundaries = SectionBoundaries.fromMarker('guide');
    const selector = new SectionContent('guide', 'x').selector;
    expect(boundaries.start).toBe('<!-- flyrail:guide:start -->');
    expect(selector.start).toBe(boundaries.start);
    expect(selector.end).toBe(boundaries.end);
  });

  test('a tree adds implied directories and sorts entries', () => {
    const tree = new TreeContent([fileEntry('b/c.txt', new Uint8Array([1])), directoryEntry('a')]);
    expect(tree.entries.map((entry) => entry.path)).toEqual(['a', 'b', 'b/c.txt']);
  });

  test('file content keeps its bytes and mode', () => {
    const file = new FileContent(new Uint8Array([1, 2]), 'executable');
    expect([...file.data]).toEqual([1, 2]);
    expect(file.mode).toBe('executable');
  });

  test('an unknown file mode is refused with the Rust text', () => {
    expect(() => new FileContent(new Uint8Array(), 'secret' as 'private')).toThrow(
      new ValueError("'secret' is not a valid FileMode"),
    );
  });

  test('an empty selector is refused', () => {
    expect(() => new Selector([])).toThrow(
      new ValueError('a selector must select a key or member'),
    );
  });

  test('a member keeps its identity and a before placement requires an anchor', () => {
    const member = new Member('review', ['name']);
    expect(member.identity).toBe('review');
    expect(new Placement('before', member).anchor).toBe(member);
    expect(() => new Placement('before')).toThrow(ValueError);
  });

  test('a schema scaffold exposes structured content', () => {
    const content = new DocumentSchema('json', '$schema', 'https://example.com/s').content();
    expect(content.selector.parts[0]).toBeInstanceOf(Key);
    expect(content.value).toBe('https://example.com/s');
  });
});

describe('content isolation', () => {
  test('mutating the bytes a file exposes does not change the file', () => {
    const file = new FileContent(new Uint8Array([1, 2, 3]), 'private');
    file.data[0] = 9;
    expect([...file.data]).toEqual([1, 2, 3]);
  });

  test('mutating the caller array after construction does not change the structured value', () => {
    const value = [1n, 2n];
    const content = new StructuredContent('json', new Selector([new Key('a')]), value);
    value.push(3n);
    expect(content.value).toEqual([1n, 2n]);
  });

  test('mutating tree entry bytes does not change the tree', () => {
    const tree = new TreeContent([fileEntry('a.txt', new Uint8Array([5]))]);
    const first = tree.entries[0]?.data;
    if (first) {
      first[0] = 7;
    }
    expect([...(tree.entries[0]?.data ?? [])]).toEqual([5]);
  });

  test('a plain key object in a selector is dispatched on its kind', () => {
    const selector = new Selector([{ kind: 'key', name: 'a' } as unknown as Key]);
    expect(selector.parts.map((part) => part.kind)).toEqual(['key']);
  });
});
