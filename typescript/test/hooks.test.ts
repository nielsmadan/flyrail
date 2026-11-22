import { describe, expect, test } from 'vitest';
import {
  EnvRef,
  HOOK_EVENT_TIMEOUT_LIMIT_MS,
  HOOK_INPUT_LIMIT,
  HOOK_JSON_DEPTH_LIMIT,
  HookConfig,
  HookEffects,
  HookError,
  HookHandler,
  HookJson,
  HookRequest,
  HookResponse,
  ValueError,
} from 'flyrail';
import { decode, encode } from './helpers/bytes.js';

const codeOf = (action: () => unknown): string | undefined => {
  try {
    action();
  } catch (error) {
    return error instanceof HookError ? error.code : undefined;
  }
  return undefined;
};

const handler = (
  id: string,
  overrides: Partial<ConstructorParameters<typeof HookHandler>[0]> = {},
) =>
  new HookHandler({
    id,
    event: 'before-tool',
    argv: ['/bin/echo', 'x'],
    cwd: '/work',
    timeoutMs: 1000,
    outcomes: ['continue', 'block'],
    tools: ['Read', 'Bash'],
    ...overrides,
  });

describe('hook JSON', () => {
  test('keeps a lone surrogate exactly', () => {
    expect(HookJson.string('\uD800').text).toBe('\uD800');
  });

  test('builds and serializes nested values', () => {
    const value = HookJson.object([
      ['b', HookJson.array([HookJson.integer(9007199254740993n), HookJson.null()])],
      ['a', HookJson.boolean(true)],
    ]);
    expect(value.isObject).toBe(true);
    expect(decode(value.bytes())).toBe('{"b":[9007199254740992,null],"a":true}');
  });

  test('parses text and reports the depth limit', () => {
    expect(HookJson.parse(encode('"x"')).text).toBe('x');
    const deep = `${'['.repeat(HOOK_JSON_DEPTH_LIMIT + 1)}0${']'.repeat(HOOK_JSON_DEPTH_LIMIT + 1)}`;
    expect(() => HookJson.parse(encode(deep))).toThrow(HookError);
    try {
      HookJson.parse(encode(deep));
    } catch (error) {
      expect((error as HookError).code).toBe('json-depth');
    }
  });

  test('rejects non-string text and an out-of-range integer', () => {
    expect(() => HookJson.string(5 as unknown as string)).toThrow(ValueError);
    expect(() => HookJson.string(undefined as unknown as string)).toThrow(ValueError);
    expect(() => HookJson.integer(2n ** 63n)).toThrow(ValueError);
  });

  test('rejects a non-bigint integer and a foreign member', () => {
    expect(() => HookJson.integer(1 as unknown as bigint)).toThrow(ValueError);
    expect(() => HookJson.array([{} as HookJson])).toThrow(ValueError);
  });

  test('accepts JSON at the input limit and refuses one byte more', () => {
    expect(HookJson.string('x'.repeat(HOOK_INPUT_LIMIT - 2)).bytes()).toHaveLength(
      HOOK_INPUT_LIMIT,
    );
    expect(codeOf(() => HookJson.string('x'.repeat(HOOK_INPUT_LIMIT - 1)))).toBe('input-limit');
  });
});

describe('hook request', () => {
  test('round-trips canonical bytes with its parts', () => {
    const request = new HookRequest({
      event: 'before-tool',
      context: HookJson.object([]),
      tool: 'Bash',
      input: HookJson.object([['command', HookJson.string('pwd')]]),
    });
    expect(decode(request.bytes())).toBe(
      '{"version":1,"event":"before-tool","context":{},"tool":"Bash","input":{"command":"pwd"}}',
    );
    const parsed = HookRequest.parse(request.bytes());
    expect(parsed.tool).toBe('Bash');
    expect(parsed.input?.isObject).toBe(true);
    expect(parsed.result).toBeNull();
  });

  test('a lone surrogate in the tool name is a host-arguments HookError', () => {
    expect(
      codeOf(
        () =>
          new HookRequest({ event: 'before-tool', context: HookJson.object([]), tool: '\uD800' }),
      ),
    ).toBe('host-arguments');
  });

  test('a tool event without a tool throws HookError with host-tool', () => {
    expect(
      codeOf(() => new HookRequest({ event: 'before-tool', context: HookJson.object([]) })),
    ).toBe('host-tool');
  });
});

describe('hook response', () => {
  test('parses a declared outcome', () => {
    const response = HookResponse.parse(
      encode('{"version":1,"outcome":"continue"}'),
      'before-tool',
      ['continue'],
    );
    expect(response.outcome).toBe('continue');
    expect(response.value).toBeNull();
  });

  test('a block response for a continue-only handler throws HookError', () => {
    const block = encode('{"version":1,"outcome":"block","reason":"no"}');
    expect(codeOf(() => HookResponse.parse(block, 'before-tool', ['continue']))).toBe(
      'undeclared-outcome',
    );
    expect(HookResponse.parse(block, 'before-tool', ['block']).value?.text).toBe('no');
  });

  test('builds block, ask, context and modify-input responses', () => {
    expect(
      HookResponse.parse(HookResponse.block('\uD800').bytes(), 'before-tool', ['block']).value
        ?.text,
    ).toBe('\uD800');
    expect(HookResponse.ask('why').outcome).toBe('ask');
    expect(HookResponse.context('note').value?.text).toBe('note');
    const replaced = HookResponse.modifyInput('before-tool', HookJson.object([]));
    expect(replaced.outcome).toBe('modify-input');
    expect(() => HookResponse.modifyInput('before-tool', HookJson.string('x'))).toThrow(HookError);
    expect(() => HookResponse.block('   ')).toThrow(HookError);
  });
});

describe('hook effects', () => {
  test('parses combined effects', () => {
    const effects = HookEffects.parse(
      encode('{"contexts":["a","\\ud800"],"decision":"ask","reason":"why"}'),
    );
    expect(effects.contexts).toEqual(['a', '\uD800']);
    expect(effects.decision).toBe('ask');
    expect(effects.reason).toBe('why');
    expect(effects.input).toBeNull();
  });

  test('none has no effects', () => {
    const none = HookEffects.none();
    expect([none.contexts, none.decision, none.reason, none.input]).toEqual([[], null, null, null]);
  });
});

describe('hook handler and config', () => {
  test('rejects an environment value that is neither a string nor an EnvRef', () => {
    for (const bad of [1, null]) {
      expect(() => handler('a', { environment: { A: bad as unknown as string } })).toThrow(
        ValueError,
      );
    }
  });

  test('normalizes outcomes, tools and environment', () => {
    const value = handler('a', {
      environment: { B: 'text', A: new EnvRef('PATH') },
    });
    expect(value.outcomes).toEqual(['block', 'continue']);
    expect(value.tools).toEqual(['Bash', 'Read']);
    expect(Object.keys(value.environment)).toEqual(['A', 'B']);
    expect(value.environment.A).toBeInstanceOf(EnvRef);
    expect(value.environment.B).toBe('text');
  });

  test('rejects an out-of-range timeout with ValueError', () => {
    expect(() => handler('a', { timeoutMs: 0 })).toThrow(ValueError);
    expect(() => handler('a', { timeoutMs: 1.5 })).toThrow(ValueError);
    expect(handler('a', { timeoutMs: HOOK_EVENT_TIMEOUT_LIMIT_MS }).timeoutMs).toBe(
      HOOK_EVENT_TIMEOUT_LIMIT_MS,
    );
    expect(() => handler('a', { timeoutMs: HOOK_EVENT_TIMEOUT_LIMIT_MS + 1 })).toThrow(
      new ValueError('hook timeout must be 1-300000 integer milliseconds'),
    );
  });

  test('a config serializes to a JSON object and orders handlers by id', () => {
    const config = new HookConfig('claude', [handler('b'), handler('a')]);
    expect(config.host).toBe('claude');
    expect(config.handlers.map((entry) => entry.id)).toEqual(['a', 'b']);
    expect(HookJson.parse(config.bytes()).isObject).toBe(true);
  });

  test('a config rejects a foreign handler', () => {
    expect(() => new HookConfig('claude', [{} as HookHandler])).toThrow(ValueError);
  });
});
