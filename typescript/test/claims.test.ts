import { mkdir, symlink } from 'node:fs/promises';
import { join } from 'node:path';
import { describe, expect, test } from 'vitest';
import {
  AuthorityError,
  Claim,
  type ClaimSelector,
  ResourceAuthority,
  Selector,
  SectionBoundaries,
  ValueError,
  WHOLE,
  canonicalDestination,
  transactionPath,
} from 'flyrail';
import { tempRoot } from './helpers/temp.js';

describe('claims', () => {
  test('a section claim given a subtree marker converts to boundaries', () => {
    const claim = new Claim('section', { kind: 'subtree', path: 'guide' });
    expect(claim.selector).toBeInstanceOf(SectionBoundaries);
    expect((claim.selector as SectionBoundaries).start).toBe('<!-- flyrail:guide:start -->');
  });

  test('a file claim with a selector is refused', () => {
    expect(() => new Claim('file', { kind: 'subtree', path: 'x' })).toThrow(ValueError);
  });

  test('claims compare by kind and selector', () => {
    expect(new Claim('file', WHOLE).equals(new Claim('file', WHOLE))).toBe(true);
    expect(new Claim('file', WHOLE).equals(new Claim('tree', WHOLE))).toBe(false);
  });
});

describe('authority', () => {
  test('canonicalDestination resolves a destination below missing directories', async () => {
    const root = await tempRoot();
    expect(await canonicalDestination(`${root}/./missing/file.md`)).toBe(
      join(root, 'missing', 'file.md'),
    );
  });

  test('canonicalDestination refuses a symlinked ancestor as unsafe', async () => {
    const root = await tempRoot();
    await mkdir(join(root, 'real'));
    await symlink(join(root, 'real'), join(root, 'link'));
    const failure: unknown = await canonicalDestination(join(root, 'link', 'file.md')).catch(
      (error: unknown) => error,
    );
    expect(failure).toBeInstanceOf(AuthorityError);
    expect((failure as AuthorityError).kind).toBe('unsafe');
    expect((failure as AuthorityError).path).toBe(join(root, 'link'));
  });

  test('a relative destination is refused before native', async () => {
    await expect(ResourceAuthority.create('relative.md')).rejects.toThrow(
      new ValueError('resource destination must be an absolute path'),
    );
  });

  test('transactionPath places the file beside the destination', () => {
    const path = transactionPath('/tmp/project/AGENTS.md', '0123456789abcdef0123456789abcdef');
    expect(path).toMatch(
      /^\/tmp\/project\/\.flyrail-[0-9a-f]{64}-0123456789abcdef0123456789abcdef\.txn$/,
    );
  });

  test('a malformed transaction id is refused', () => {
    expect(() => transactionPath('/tmp/x', 'abc')).toThrow(ValueError);
  });
});

describe('claims with structurally typed inputs', () => {
  test('a plain section selector object dispatches on its kind', () => {
    const claim = new Claim('section', { kind: 'section', start: '<!-- a -->', end: '<!-- b -->' });
    expect(claim.selector).toBeInstanceOf(SectionBoundaries);
    expect((claim.selector as SectionBoundaries).end).toBe('<!-- b -->');
  });

  test('a plain structured selector object keeps its parts', () => {
    const claim = new Claim('structured', {
      kind: 'structured',
      parts: [{ kind: 'key', name: 'a' }],
    } as unknown as Selector);
    expect((claim.selector as Selector).parts.map((part) => part.kind)).toEqual(['key']);
  });

  test('an unknown selector kind is a ValueError', () => {
    expect(() => new Claim('file', { kind: 'bogus' } as unknown as ClaimSelector)).toThrow(
      ValueError,
    );
  });

  test('a plain object is not an authority', () => {
    const claim = new Claim('file', WHOLE);
    expect(() => claim.identity({ destination: '/x' })).toThrow(ValueError);
  });
});
