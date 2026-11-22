import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, test } from 'vitest';

interface NapiFunction {
  readonly location: string;
  readonly catchUnwind: boolean;
}

const nativeSource = fileURLToPath(new URL('../native/src/', import.meta.url));
const attributeStart = /#\[(?:napi_derive::)?napi\b/g;
const leading = /^(?:\s|\/\/[^\n]*\n|#\[(?:[^[\]]|\[[^\]]*\])*\])*/;
const functionItem =
  /^(?:pub(?:\([^)]*\))?\s+)?(?:const\s+)?(?:async\s+)?(?:unsafe\s+)?(?:extern\s+"[^"]*"\s+)?fn\b/;

const blankStrings = (source: string): string =>
  source.replace(
    /"(?:[^"\\]|\\.)*"/g,
    (literal) => `"${literal.slice(1, -1).replace(/[^\n]/g, ' ')}"`,
  );

const closingParen = (source: string, open: number): number => {
  let depth = 0;
  for (let index = open; index < source.length; index += 1) {
    depth += source[index] === '(' ? 1 : source[index] === ')' ? -1 : 0;
    if (depth === 0) {
      return index;
    }
  }
  return -1;
};

const topLevelOptions = (inner: string): string[] => {
  const options: string[] = [];
  let depth = 0;
  let current = '';
  for (const char of inner) {
    if (char === ',' && depth === 0) {
      options.push(current.trim());
      current = '';
      continue;
    }
    depth += '([<'.includes(char) ? 1 : ')]>'.includes(char) ? -1 : 0;
    current += char;
  }
  options.push(current.trim());
  return options;
};

const scan = (file: string, original: string): NapiFunction[] => {
  const source = blankStrings(original);
  return [...source.matchAll(attributeStart)].flatMap((match) => {
    const line = source.slice(0, match.index).split('\n').length;
    const location = `${file}:${line.toString()}`;
    const afterName = match.index + match[0].length;
    let inner = '';
    let end = afterName;
    if (source[afterName] === '(') {
      const close = closingParen(source, afterName);
      inner = close < 0 ? '' : source.slice(afterName + 1, close);
      end = close + 1;
    }
    if (end <= 0 || source[end] !== ']') {
      throw new Error(`cannot parse the napi attribute at ${location}`);
    }
    const item = source.slice(end + 1).replace(leading, '');
    if (!functionItem.test(item)) {
      return [];
    }
    return [{ location, catchUnwind: topLevelOptions(inner).includes('catch_unwind') }];
  });
};

const files = readdirSync(nativeSource, { recursive: true, encoding: 'utf8' })
  .filter((file) => file.endsWith('.rs'))
  .sort();
const scanned = files
  .map((file) => [file, scan(file, readFileSync(join(nativeSource, file), 'utf8'))] as const)
  .filter(([, functions]) => functions.length > 0);

describe('napi export attributes', () => {
  test('the scan finds the exported functions, methods and constructors', () => {
    const locations = scanned.flatMap(([, functions]) => functions.map((f) => f.location));
    expect(locations).toEqual(
      expect.arrayContaining([
        expect.stringMatching(/^cancel\.rs:/),
        expect.stringMatching(/^lib\.rs:/),
        expect.stringMatching(/^internal\.rs:/),
      ]),
    );
  });

  test.each(scanned)('every export in %s catches unwinds', (_file, functions) => {
    const offenders = functions.filter((f) => !f.catchUnwind).map((f) => f.location);
    expect(offenders).toEqual([]);
  });

  test('the scan reports functions, methods and constructors that lack catch_unwind', () => {
    const source = [
      '#[napi]',
      'pub struct Token {}',
      '#[napi]',
      'impl Token {',
      '    #[napi(constructor)]',
      '    pub fn new() -> Self { Self {} }',
      '    #[napi(',
      '        js_name = "other",',
      '        catch_unwind,',
      '        ts_return_type = "Record<string, catch_unwind>"',
      '    )]',
      '    pub fn other(&self) {}',
      '}',
      '#[napi(js_name = "bare", ts_return_type = "Promise<catch_unwind, x>")]',
      '#[expect(clippy::panic)]',
      'pub fn bare() {}',
    ].join('\n');
    expect(scan('sample.rs', source)).toEqual([
      { location: 'sample.rs:5', catchUnwind: false },
      { location: 'sample.rs:7', catchUnwind: true },
      { location: 'sample.rs:14', catchUnwind: false },
    ]);
  });

  test('the scan handles nested function types and the qualified napi path', () => {
    const source = [
      '#[napi(js_name = "x", ts_args_type = "cb: (a: (b: number) => void) => void")]',
      'pub fn nested() {}',
      '#[napi_derive::napi(js_name = "y")]',
      'pub fn qualified() {}',
      '#[napi_derive::napi(js_name = "z", catch_unwind)]',
      'pub fn guarded() {}',
    ].join('\n');
    expect(scan('sample.rs', source)).toEqual([
      { location: 'sample.rs:1', catchUnwind: false },
      { location: 'sample.rs:3', catchUnwind: false },
      { location: 'sample.rs:5', catchUnwind: true },
    ]);
  });

  test('the scan fails on an attribute it cannot parse', () => {
    expect(() => scan('sample.rs', '#[napi(js_name = "x"\npub fn broken() {}')).toThrow(
      'cannot parse the napi attribute at sample.rs:1',
    );
  });
});
