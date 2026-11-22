import { describe, expect, test } from 'vitest';
import { ValueError } from '../src/errors.js';
import { optionalAbsolute, requireAbsolute } from '../src/internal/paths.js';

describe('requireAbsolute', () => {
  test('rejects a relative path with the labelled message', () => {
    expect(() => requireAbsolute('rel', 'root')).toThrow(
      new ValueError('root must be an absolute path'),
    );
  });

  test('returns an absolute path unchanged', () => {
    expect(requireAbsolute('/srv/project', 'root')).toBe('/srv/project');
  });

  test('rejects a path that is not well-formed text', () => {
    expect(() => requireAbsolute('/srv/\uD800', 'root')).toThrow(ValueError);
  });
});

describe('optionalAbsolute', () => {
  test.each([null, undefined])('maps %s to null', (path) => {
    expect(optionalAbsolute(path, 'home')).toBeNull();
  });

  test('validates a present path', () => {
    expect(() => optionalAbsolute('home', 'home')).toThrow(
      new ValueError('home must be an absolute path'),
    );
  });
});
