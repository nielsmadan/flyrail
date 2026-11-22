import { describe, expect, test } from 'vitest';
import {
  Bundle,
  BundleIdentity,
  DocumentRequest,
  Claim,
  HookJson,
  InstructionArtifact,
  NativePanicError,
  RenderContext,
  SectionContent,
  Target,
  WHOLE,
  decodeValue,
  discoverLoadout,
  editDocument,
  encodeValue,
  fileEntry,
  inspectInstallation,
  install,
  uninstall,
  MemoryStore,
  remove,
  sync,
  instructionDestination,
  render,
} from 'flyrail';
import { runMutation } from '../src/internal/abort.js';
import { awaitNative } from '../src/internal/call.js';
import { loadedFrom, native } from '../src/native/loader.js';
import { join } from 'node:path';
import { codexProject, holdLock, storeUnder } from './helpers/lifecycle.js';
import { tempRoot } from './helpers/temp.js';

describe('loader', () => {
  test('loads the native addon', () => {
    expect(loadedFrom).toMatch(/flyrail\.[a-z0-9-]+\.node$/);
    expect(native.nativeVersion()).toMatch(/^\d+\.\d+\.\d+$/);
  });
});

describe('pool', () => {
  test('runs concurrent jobs and survives a worker panic', async () => {
    await expect(
      Promise.all([
        awaitNative(native.internalRendezvous(2, 5000)),
        awaitNative(native.internalRendezvous(2, 5000)),
      ]),
    ).resolves.toEqual([true, true]);
    await expect(awaitNative(native.internalPanicAsync())).rejects.toBeInstanceOf(NativePanicError);
  });
});

describe('abort', () => {
  test('an aborted mutation rejects with the signal reason', async () => {
    const controller = new AbortController();
    const reason = new Error('stop');
    const outcome = runMutation(controller.signal, (token) =>
      awaitNative(native.internalBlock(5000, token)),
    );
    controller.abort(reason);
    await expect(outcome).rejects.toBe(reason);
  });
});

describe('values', () => {
  test('encodes and decodes a value', () => {
    expect(decodeValue(encodeValue([1n, 'x', null]))).toEqual([1n, 'x', null]);
  });
});

describe('bundles', () => {
  test('loads a bundle from memory and reads its digest', () => {
    const manifest = JSON.stringify({
      schema_version: 1,
      id: 'tiny',
      version: '1',
      skills: [{ name: 'a', path: 'a' }],
    });
    const bundle = Bundle.fromMemory(new TextEncoder().encode(manifest), [
      fileEntry('a/SKILL.md', new Uint8Array()),
    ]);
    expect(bundle.contentDigest).toBe(
      'f614cdad442379913268af2f5ae845915ad12ff99086dc91e65f2a7e5b73fdb6',
    );
  });
});

describe('artifacts, content and claims', () => {
  test('builds a bundle from an artifact and compares claims', () => {
    const bundle = Bundle.fromArtifacts(new BundleIdentity('tiny', '1'), [
      new InstructionArtifact('guide', 'text'),
    ]);
    expect(bundle.artifacts[0]).toBeInstanceOf(InstructionArtifact);
    expect(new Claim('file', WHOLE).overlaps(new Claim('tree', WHOLE))).toBe(true);
  });
});

describe('targets and rendering', () => {
  test('renders an instruction bundle for codex', () => {
    const context = new RenderContext(Target.project('codex', '/work/app'), 'linux');
    const bundle = Bundle.fromArtifacts(new BundleIdentity('tiny', '1'), [
      new InstructionArtifact('guide', 'text'),
    ]);
    expect(instructionDestination(context)).toBe('/work/app/AGENTS.md');
    expect(render(bundle, context).artifacts.map((artifact) => artifact.destination)).toEqual([
      '/work/app/AGENTS.md',
    ]);
  });
});

describe('editing', () => {
  test('creates a section in an absent document', () => {
    const edit = editDocument(null, [DocumentRequest.set(new SectionContent('guide', 'x'))]);
    expect(new TextDecoder().decode(edit.after ?? new Uint8Array())).toBe(
      '<!-- flyrail:guide:start -->\nx\n<!-- flyrail:guide:end -->\n',
    );
  });
});

describe('hooks', () => {
  test('parses hook JSON', () => {
    expect(HookJson.parse(new TextEncoder().encode('{"a":[1]}')).isObject).toBe(true);
  });
});

describe('skills', () => {
  test('installs and uninstalls a skill', async () => {
    const root = await tempRoot();
    const manifest = JSON.stringify({
      schema_version: 1,
      id: 'tiny',
      version: '1',
      skills: [{ name: 'a', path: 'a' }],
    });
    const bundle = Bundle.fromMemory(new TextEncoder().encode(manifest), [
      fileEntry('a/SKILL.md', new TextEncoder().encode('# A\n')),
    ]);
    const targets = [Target.directory(join(root, 'skills'))];
    const store = new MemoryStore();
    expect((await install(bundle, targets, store))[0]?.status).toBe('applied');
    expect((await uninstall('tiny', targets, store))[0]?.status).toBe('applied');
  });
});

describe('lifecycle', () => {
  test('syncs, inspects and removes an instruction bundle', async () => {
    const root = await tempRoot();
    const { bundle, rendered, target } = await codexProject(root, await storeUnder(root));
    expect((await sync(bundle, rendered, target)).status).toBe('applied');
    expect((await inspectInstallation(bundle.id, target)).matches(bundle, rendered)).toBe(true);
    expect((await remove(bundle.id, target)).status).toBe('applied');
  });

  test('an abort behind a held index lock rejects with the reason', async () => {
    const root = await tempRoot();
    const store = await storeUnder(root);
    const { bundle, rendered, target } = await codexProject(join(root, 'app'), store, 'team');
    const hold = await holdLock(store, `index/team/${target.contextDigest}`);
    try {
      const controller = new AbortController();
      const reason = new Error('stop');
      const outcome = sync(bundle, rendered, target, {
        lockTimeoutMs: 10_000,
        signal: controller.signal,
      });
      controller.abort(reason);
      await expect(outcome).rejects.toBe(reason);
    } finally {
      hold.release();
    }
  });
});

describe('loadout', () => {
  test('discovers an absent registration in an empty home', async () => {
    const home = await tempRoot();
    expect((await discoverLoadout({ home, environment: {} })).status).toBe('absent');
  });
});
