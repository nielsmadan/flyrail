import { spawn } from 'node:child_process';
import { accessSync, constants } from 'node:fs';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, test } from 'vitest';
import {
  Bundle,
  BundleIdentity,
  DirectoryStore,
  InstallationTarget,
  InstructionArtifact,
  RenderContext,
  Target,
  inspectInstallation,
  recoverInstallation,
  remove,
  render,
  sync,
} from 'flyrail';
import { canonicalState } from './helpers/conformance.js';
import { fixture, fixturesRequired } from './helpers/fixtures.js';
import { encodeRecord, relocated } from './helpers/state-records.js';
import { tempRoot } from './helpers/temp.js';
import { locateTool } from './helpers/tools.js';

const driverTimeoutMs = 60_000;
const testTimeoutMs = 120_000;

const rustDriver = fileURLToPath(
  new URL('../../rust/target/debug/examples/interop', import.meta.url),
);
const pythonDriver = fileURLToPath(
  new URL('../../python/scripts/interop_driver.py', import.meta.url),
);

const executable = (path: string): boolean => {
  try {
    accessSync(path, constants.X_OK);
    return true;
  } catch {
    return false;
  }
};

const driversAvailable = (): boolean => {
  const rust = executable(rustDriver);
  const python = locateTool('python') !== null;
  if (rust && python) {
    return true;
  }
  const missing = [rust ? '' : 'the Rust interop driver', python ? '' : 'the Python interpreter']
    .filter((name) => name !== '')
    .join(' and ');
  if (fixturesRequired()) {
    throw new Error(`${missing} is required: run just interop-driver and just setup`);
  }
  console.warn(`skipping: ${missing} is not available`);
  return false;
};

interface Run {
  readonly stdout: Buffer;
  readonly stderr: string;
  readonly code: number | null;
}

const run = (command: string, args: readonly string[], input?: Uint8Array): Promise<Run> =>
  new Promise((resolve, reject) => {
    const child = spawn(command, [...args], {
      stdio: ['pipe', 'pipe', 'pipe'],
      timeout: driverTimeoutMs,
    });
    const out: Buffer[] = [];
    const err: Buffer[] = [];
    child.stdout.on('data', (chunk: Buffer) => out.push(chunk));
    child.stderr.on('data', (chunk: Buffer) => err.push(chunk));
    child.on('error', reject);
    child.on('close', (code, signal) => {
      if (signal !== null) {
        reject(new Error(`${command} ${args.join(' ')} ended with ${signal}`));
        return;
      }
      resolve({ stdout: Buffer.concat(out), stderr: Buffer.concat(err).toString('utf8'), code });
    });
    child.stdin.on('error', () => undefined);
    child.stdin.end(input);
  });

interface Driver {
  readonly json: (...args: readonly string[]) => Promise<DriverJson>;
  readonly bytes: (args: readonly string[], input: Uint8Array) => Promise<Buffer>;
}

interface DriverJson {
  readonly status?: string;
  readonly is_current?: boolean;
  readonly version?: string | null;
  readonly pending?: boolean;
  readonly resources: readonly { readonly claims?: readonly unknown[] }[];
  readonly error: unknown;
}

const succeeded = (command: string, args: readonly string[], result: Run): void => {
  if (result.code !== 0) {
    throw new Error(
      `${command} ${args.join(' ')} exited with ${String(result.code)}: ${result.stderr}`,
    );
  }
};

const driver = (command: string, prefix: readonly string[]): Driver => ({
  json: async (...args) => {
    const result = await run(command, [...prefix, ...args]);
    succeeded(command, [...prefix, ...args], result);
    return JSON.parse(result.stdout.toString('utf8')) as DriverJson;
  },
  bytes: async (args, input) => {
    const result = await run(command, [...prefix, ...args], input);
    succeeded(command, [...prefix, ...args], result);
    return result.stdout;
  },
});

const rust = driver(rustDriver, []);
const python = (): Driver => driver(locateTool('python') as string, ['-I', '-B', pythonDriver]);

const guideBundle = (id: string, text: string): Bundle =>
  Bundle.fromArtifacts(new BundleIdentity(id, '1'), [new InstructionArtifact('guide', text)]);

interface App {
  readonly project: string;
  readonly state: string;
  readonly target: InstallationTarget;
  readonly bundle: Bundle;
  readonly rendered: ReturnType<typeof render>;
}

async function app(root: string, id: string, text: string, state = 'state'): Promise<App> {
  const project = join(root, 'project');
  await mkdir(project, { recursive: true, mode: 0o700 });
  const store = await DirectoryStore.open(join(root, state));
  const bundle = guideBundle(id, text);
  const context = new RenderContext(Target.project('codex', project), 'linux');
  return {
    project,
    state: join(root, state),
    target: new InstallationTarget(store),
    bundle,
    rendered: render(bundle, context),
  };
}

const stateArgs = (state: string, id: string): readonly string[] => [
  '--state',
  state,
  '--bundle-id',
  id,
];

describe.skipIf(!driversAvailable())('Rust and Python interop', () => {
  test(
    'a TypeScript installation is current and removable through the Rust driver',
    { timeout: testTimeoutMs },
    async () => {
      const root = await tempRoot();
      const { state, target, bundle, rendered } = await app(root, 'team', 'Shared guidance.\n');
      expect((await sync(bundle, rendered, target)).status).toBe('applied');

      const observed = await rust.json('inspect', ...stateArgs(state, 'team'));
      expect(observed.is_current).toBe(true);
      expect((await rust.json('remove', ...stateArgs(state, 'team'))).status).toBe('applied');

      const after = await inspectInstallation('team', target);
      expect(after.version).toBeNull();
      expect(after.resources).toHaveLength(0);
    },
  );

  test(
    'a Rust installation is current and removable through TypeScript',
    { timeout: testTimeoutMs },
    async () => {
      const root = await tempRoot();
      const { project, state, target } = await app(root, 'team', 'unused\n');
      const source = join(root, 'source');
      await mkdir(source, { recursive: true, mode: 0o700 });
      await writeFile(
        join(source, 'flyrail.json'),
        JSON.stringify({
          schema_version: 2,
          id: 'team',
          version: '1',
          artifacts: [{ kind: 'instruction', id: 'guide', path: 'instructions.md' }],
        }),
      );
      await writeFile(join(source, 'instructions.md'), 'Shared guidance.\n');

      const installed = await rust.json(
        'install',
        ...stateArgs(state, 'team'),
        '--source',
        source,
        '--destination',
        project,
        '--agent',
        'codex',
      );
      expect(installed.status).toBe('applied');

      const observed = await inspectInstallation('team', target);
      expect(observed.isCurrent).toBe(true);
      expect((await remove('team', target)).status).toBe('applied');
      expect((await rust.json('inspect', ...stateArgs(state, 'team'))).resources).toHaveLength(0);
    },
  );

  test(
    'a Rust update that stops before the index completes is pending in TypeScript and recovered',
    { timeout: testTimeoutMs },
    async () => {
      const root = await tempRoot();
      const { project, state, target } = await app(root, 'team', 'unused\n');
      const destination = join(project, 'guide.txt');
      const first = await rust.json(
        'write',
        ...stateArgs(state, 'team'),
        '--destination',
        destination,
        '--payload',
        'first',
        '--acquisition',
        'takeover',
      );
      expect(first.status).toBe('applied');
      const partial = await rust.json(
        'update',
        ...stateArgs(state, 'team'),
        '--destination',
        destination,
        '--version',
        '2',
        '--payload',
        'second',
        '--fail-index-completion',
      );
      expect(partial.status).toBe('partial');

      const pending = await inspectInstallation('team', target);
      expect(pending.pending).toBe(true);
      expect(pending.version).toBe('1');
      expect((await recoverInstallation('team', target)).succeeded).toBe(true);

      const recovered = await rust.json('inspect', ...stateArgs(state, 'team'));
      expect(recovered.pending).toBe(true);
      expect(recovered.version).toBe('1');
      expect(await readFile(destination, 'utf8')).toBe('second');
    },
  );

  test(
    'TypeScript and Python sections share one document and leave each other intact',
    { timeout: testTimeoutMs },
    async () => {
      const root = await tempRoot();
      const typescript = await app(root, 'ts-app', 'TypeScript guidance.\n', 'state-a');
      const document = join(typescript.project, 'AGENTS.md');
      const original = 'Existing notes.\n';
      await writeFile(document, original);

      expect((await sync(typescript.bundle, typescript.rendered, typescript.target)).status).toBe(
        'applied',
      );
      const withTypescript = await readFile(document, 'utf8');
      expect(withTypescript).toContain('TypeScript guidance.');

      const stateB = join(root, 'state-b');
      const installed = await python().json(
        'write',
        ...stateArgs(stateB, 'py-app'),
        '--destination',
        document,
        '--section',
        'py',
        '--payload',
        'Python guidance.\n',
      );
      expect(installed.status).toBe('applied');
      const both = await readFile(document, 'utf8');
      expect(both).toContain('TypeScript guidance.');
      expect(both).toContain('Python guidance.');

      expect((await python().json('remove', ...stateArgs(stateB, 'py-app'))).status).toBe(
        'applied',
      );
      expect(await readFile(document, 'utf8')).toBe(withTypescript);

      expect((await remove('ts-app', typescript.target)).status).toBe('applied');
      expect(await readFile(document, 'utf8')).toBe(original);
    },
  );

  const receipts =
    (fixture('configurations.json') as { readonly receipts?: readonly Named[] } | undefined)
      ?.receipts ?? [];

  interface Named {
    readonly name: string;
    readonly record: unknown;
  }

  test.skipIf(receipts.length === 0 && !fixturesRequired())('the fixtures hold receipts', () => {
    expect(receipts.length).toBeGreaterThan(0);
  });

  test.each(receipts.map((entry) => [entry.name, entry] as const))(
    'receipt %s canonicalizes to the Rust driver bytes',
    { timeout: testTimeoutMs },
    async (_, entry) => {
      const root = await tempRoot();
      const input = encodeRecord(await relocated(entry.record, root));
      const fromRust = await rust.bytes(['encode', '--type', 'Receipt'], input);
      expect(fromRust.equals(Buffer.from(canonicalState('Receipt', input)))).toBe(true);
    },
  );
});
