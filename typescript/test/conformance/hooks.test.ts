import { describe, expect, test } from 'vitest';
import { HookRequest, HookResponse, type HookEvent, type HookOutcome } from 'flyrail';
import { fixture } from '../helpers/fixtures.js';

interface HookVector {
  readonly name: string;
  readonly event: HookEvent;
  readonly context: unknown;
  readonly canonical: Readonly<Record<string, unknown>>;
  readonly response: { readonly outcome: HookOutcome };
}

const document = fixture('hooks.json') as
  { readonly version: number; readonly cases: readonly HookVector[] } | undefined;

const declared: readonly HookOutcome[] = ['continue', 'block', 'ask', 'modify-input', 'context'];
const encode = (text: string): Uint8Array => new TextEncoder().encode(text);
const decode = (bytes: Uint8Array): string => new TextDecoder().decode(bytes);

describe('hooks.json', () => {
  test.skipIf(document === undefined)('is version 1 with cases', () => {
    expect(document?.version).toBe(1);
    expect(document?.cases.length).toBeGreaterThan(0);
  });

  test.each(document?.cases ?? [])('$name', (vector) => {
    const parts = ['tool', 'input', 'result'].filter((name) => name in vector.canonical);
    const canonical = `{"version":1,"event":${JSON.stringify(vector.event)},"context":${JSON.stringify(vector.context)}${parts
      .map((name) => `,"${name}":${JSON.stringify(vector.canonical[name])}`)
      .join('')}}`;
    const reordered = `{${parts
      .map((name) => `"${name}":${JSON.stringify(vector.canonical[name])},`)
      .join(
        '',
      )}"context":${JSON.stringify(vector.context)},"event":${JSON.stringify(vector.event)},"version":1}`;
    for (const source of [canonical, reordered]) {
      expect(decode(HookRequest.parse(encode(source)).bytes())).toBe(canonical);
    }
    const response = JSON.stringify(vector.response);
    const parsed = HookResponse.parse(encode(response), vector.event, declared);
    expect(decode(parsed.bytes())).toBe(response);
    expect(parsed.outcome).toBe(vector.response.outcome);
  });
});
