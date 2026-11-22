import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, test } from 'vitest';
import * as flyrail from 'flyrail';

const apiDoc = readFileSync(fileURLToPath(new URL('../docs/api.md', import.meta.url)), 'utf8');
const codeSpans = [...apiDoc.matchAll(/`([^`\n]+)`/g)].map((match) => match[1] ?? '');

describe('docs/api.md', () => {
  test.each(Object.keys(flyrail))('documents the export %s', (name) => {
    expect(codeSpans.some((span) => new RegExp(`^(new )?${name}([(.<]|$)`).test(span))).toBe(true);
  });
});
