import { chmod, mkdir, mkdtemp, rm } from 'node:fs/promises';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  Bundle,
  BundleIdentity,
  DirectoryStore,
  InstructionArtifact,
  RenderContext,
  Target,
  render,
  sync,
} from '../../src/index.js';
import type { InstallationTarget, RenderedBundle } from '../../src/index.js';
import { awaitNative } from '../../src/internal/call.js';
import { native } from '../../src/native/loader.js';
import { storeHandle } from '../../src/stores.js';

interface Project {
  readonly bundle: Bundle;
  readonly rendered: RenderedBundle;
  readonly target: InstallationTarget;
}

const base = fileURLToPath(new URL('../../../.cache/typescript/tmp/', import.meta.url));

const project = async (root: string, store: DirectoryStore, id: string): Promise<Project> => {
  await mkdir(root, { recursive: true, mode: 0o700 });
  const bundle = Bundle.fromArtifacts(new BundleIdentity(id, '1'), [
    new InstructionArtifact('guide', 'Run the project checks.\n'),
  ]);
  const context = new RenderContext(Target.project('codex', root), 'linux');
  return { bundle, rendered: render(bundle, context), target: context.installation(store) };
};

const check = (label: string, passed: boolean): boolean => {
  console.log(`${passed ? 'ok' : 'FAIL'} ${label}`);
  return passed;
};

const concurrentSyncs = async (root: string, store: DirectoryStore): Promise<boolean> => {
  const projects = await Promise.all(
    Array.from({ length: 8 }, (_, index) =>
      project(join(root, `app-${String(index)}`), store, 'guide'),
    ),
  );
  const results = await Promise.all(
    projects.map(({ bundle, rendered, target }) =>
      sync(bundle, rendered, target, { lockTimeoutMs: 10_000 }),
    ),
  );
  return check(
    '8 concurrent syncs apply',
    results.every((result) => result.status === 'applied'),
  );
};

const abortedSync = async (root: string, store: DirectoryStore): Promise<boolean> => {
  const { bundle, rendered, target } = await project(join(root, 'held'), store, 'team');
  const hold = await awaitNative(
    native.internalHoldLock(storeHandle(store), `index/team/${target.contextDigest}`),
  );
  try {
    const controller = new AbortController();
    const reason = new Error('stop');
    setTimeout(() => {
      controller.abort(reason);
    }, 100);
    const outcome = await sync(bundle, rendered, target, {
      lockTimeoutMs: 10_000,
      signal: controller.signal,
    }).then(
      () => 'resolved',
      (error: unknown) => (error === reason ? 'reason' : 'other'),
    );
    return check(
      'an aborted sync behind a held lock rejects with the reason',
      outcome === 'reason',
    );
  } finally {
    hold.release();
  }
};

await mkdir(base, { recursive: true });
const root = await mkdtemp(join(base, 'bun-smoke-'));
await chmod(root, 0o700);
try {
  const store = await DirectoryStore.open(join(root, 'state'));
  const passed = [await concurrentSyncs(root, store), await abortedSync(root, store)];
  process.exitCode = passed.every(Boolean) ? 0 : 1;
} finally {
  await rm(root, { recursive: true, force: true });
}
