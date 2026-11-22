import { execFile } from 'node:child_process';
import { mkdir, writeFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { describe, expect, test } from 'vitest';
import { HookRuntime, type HookEvent } from 'flyrail';
import { fixture } from './helpers/fixtures.js';
import { tempRoot } from './helpers/temp.js';
import { requireTool } from './helpers/tools.js';

const run = promisify(execFile);
const harness = fileURLToPath(
  new URL('../../rust/tests/hook_runtime_harness.mjs', import.meta.url),
);
const callTimeout = 30_000;

interface Vector {
  readonly name: string;
  readonly host: string;
  readonly event: HookEvent;
  readonly context: Readonly<Record<string, unknown>>;
  readonly canonical: Readonly<Record<string, unknown>>;
  readonly expected: unknown;
}

const base64 = (text: string): string => Buffer.from(text).toString('base64');

async function invoke(
  binary: string,
  request: Readonly<Record<string, unknown>>,
): Promise<Record<string, unknown>> {
  const child = run(binary, [harness], { timeout: callTimeout, maxBuffer: 16 * 1024 * 1024 });
  child.child.stdin?.end(JSON.stringify(request));
  const { stdout, stderr } = await child;
  expect(stderr).toBe('');
  return JSON.parse(stdout) as Record<string, unknown>;
}

async function renderedRuntime(node: string): Promise<string> {
  const root = await tempRoot();
  const runtime = new HookRuntime({ node, shell: 'posix' });
  await Promise.all(
    runtime.resources.map(async (entry) => {
      if (entry.data !== null) {
        const path = join(root, entry.path);
        await mkdir(dirname(path), { recursive: true });
        await writeFile(path, entry.data);
      }
    }),
  );
  return root;
}

const expectedPacket = (vector: Vector, root: string): string => {
  const ordered = (keys: readonly string[]): [string, unknown][] =>
    keys.filter((key) => key in vector.context).map((key) => [key, vector.context[key]]);
  const context: readonly (readonly [string, unknown])[] = (() => {
    switch (vector.host) {
      case 'pi':
        return [
          ['cwd', root],
          ['execution_mode', 'print'],
          ['has_ui', false],
          ...ordered(['tool_call_id', 'source']),
        ];
      case 'opencode':
        return [['cwd', root], ...ordered(['session_id', 'tool_call_id'])];
      case 'copilot':
        return ordered([
          'cwd',
          'session_id',
          'timestamp',
          'stop_hook_active',
          'stop_reason',
          'tool_call_id',
        ]);
      default:
        return Object.entries(vector.context);
    }
  })();
  const members: [string, unknown][] = [
    ['version', 1],
    ['event', vector.event],
    ['context', Object.fromEntries(context)],
    ...['tool', 'input', 'result']
      .filter((key) => key in vector.canonical)
      .map((key): [string, unknown] => [key, vector.canonical[key]]),
  ];
  return JSON.stringify(Object.fromEntries(members));
};

describe('rendered hook runtime', () => {
  const vectors = fixture('hooks.json') as { readonly cases: readonly Vector[] } | undefined;

  test.skipIf(vectors === undefined)(
    'all shared protocol vectors execute through the rendered runtime',
    async () => {
      const node = requireTool('node');
      const bun = requireTool('bun');
      const document = vectors as { readonly cases: readonly Vector[] };
      expect(document.cases.length).toBeGreaterThan(0);
      const root = await renderedRuntime(node);
      const capture = join(root, 'capture.json');
      for (const vector of document.cases) {
        const result = await invoke(vector.host === 'opencode' ? bun : node, {
          action: 'case',
          runtime: root,
          capture,
          cwd: root,
          node,
          case: vector,
        });
        expect(result.observed, vector.name).toEqual(vector.expected);
        expect(
          Buffer.from(result.packetBase64 as string, 'base64').toString(),
          `${vector.name} packet`,
        ).toBe(expectedPacket(vector, root));
      }
    },
    10 * 60_000,
  );

  test('the library and the rendered runtime agree on UTF-16 JSON bytes', async () => {
    const node = requireTool('node');
    const root = await renderedRuntime(node);
    const { HookJson } = await import('flyrail');
    for (const source of ['"\\ud800"', '"\\ud83d\\ude00"', '{"\\ud800":"\\udfff"}', '1e21', '-0']) {
      const result = await invoke(node, {
        action: 'parse',
        runtime: root,
        data: base64(source),
      });
      expect(
        Buffer.from(result.bytesBase64 as string, 'base64').equals(
          Buffer.from(HookJson.parse(new TextEncoder().encode(source)).bytes()),
        ),
        source,
      ).toBe(true);
    }
  });
});
