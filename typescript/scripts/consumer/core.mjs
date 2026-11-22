import { access, mkdir, rename, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { join } from 'node:path';
import {
  Bundle,
  BundleIdentity,
  DirectoryStore,
  InstructionArtifact,
  RenderContext,
  Target,
  inspect,
  install,
  remove,
  render,
  sync,
  uninstall,
} from 'flyrail';

const handshakeTimeoutMs = 60_000;
const lockTimeoutMs = 10_000;

const exists = (path) =>
  access(path).then(
    () => true,
    () => false,
  );

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

const waitFor = async (path) => {
  const deadline = Date.now() + handshakeTimeoutMs;
  while (!(await exists(path))) {
    if (Date.now() > deadline) {
      throw new Error(`timed out waiting for ${path}`);
    }
    await sleep(20);
  }
};

const announce = async (directory, name, value) => {
  const staged = join(directory, `${name}.tmp`);
  await writeFile(staged, JSON.stringify(value));
  await rename(staged, join(directory, name));
};

const loadedFrom = () => {
  try {
    const objects = process.report?.getReport?.()?.sharedObjects ?? [];
    return objects.find((path) => path.endsWith('flyrail.node')) ?? null;
  } catch {
    return null;
  }
};

const project = async (root, store, id) => {
  await mkdir(root, { recursive: true, mode: 0o700 });
  const bundle = Bundle.fromArtifacts(new BundleIdentity(id, '1'), [
    new InstructionArtifact('guide', 'Run the project checks.\n'),
  ]);
  const context = new RenderContext(Target.project('codex', root), 'linux');
  return { bundle, rendered: render(bundle, context), target: context.installation(store) };
};

const blockedSync = async ({ bundle, rendered, target }) => {
  const controller = new AbortController();
  const reason = new Error('package QA abort');
  const started = performance.now();
  setTimeout(() => {
    controller.abort(reason);
  }, 100);
  const elapsedMs = () => Math.round(performance.now() - started);
  return sync(bundle, rendered, target, { lockTimeoutMs, signal: controller.signal }).then(
    (result) => ({ outcome: 'resolved', error: result.status, elapsedMs: elapsedMs() }),
    (error) => ({
      outcome: error === reason ? 'reason' : 'other',
      error: error === reason ? null : String(error),
      elapsedMs: elapsedMs(),
    }),
  );
};

export async function runConsumer(scratch, runtime) {
  const handshake = join(scratch, 'handshake');
  await mkdir(handshake, { recursive: true, mode: 0o700 });
  const store = await DirectoryStore.open(join(scratch, 'state'));
  const skills = [Target.directory(join(scratch, 'skills'))];

  const bundle = await Bundle.fromDirectory(join(scratch, 'bundle'));
  const installed = await install(bundle, skills, store, { lockTimeoutMs });
  const inspected = await inspect(bundle, skills, store);

  const projects = await Promise.all(
    Array.from({ length: 8 }, (_, index) =>
      project(join(scratch, 'projects', `app-${String(index)}`), store, 'guide'),
    ),
  );
  const concurrent = await Promise.all(
    projects.map(({ bundle: guide, rendered, target }) =>
      sync(guide, rendered, target, { lockTimeoutMs }),
    ),
  );

  const held = await project(join(scratch, 'projects', 'held'), store, 'team');
  const first = await sync(held.bundle, held.rendered, held.target, { lockTimeoutMs });
  const lockName = `index/${held.bundle.id}/${held.target.contextDigest}`;
  await announce(handshake, 'lock.json', { lockName, storeLocation: store.location });
  await waitFor(join(handshake, 'held'));
  const aborted = await blockedSync(held);
  await announce(handshake, 'aborted.json', aborted);
  await waitFor(join(handshake, 'released'));
  const afterRelease = await sync(held.bundle, held.rendered, held.target, { lockTimeoutMs });
  const removed = await remove(held.bundle.id, held.target, { lockTimeoutMs });

  const uninstalled = await uninstall(bundle.id, skills, store, { lockTimeoutMs });
  const skillFile = join(scratch, 'skills', bundle.skills[0]?.name ?? '', 'SKILL.md');

  return {
    runtime,
    versions: {
      node: process.versions.node,
      bun: process.versions.bun ?? null,
      electron: process.versions.electron ?? null,
    },
    flyrailVersion: createRequire(import.meta.url)('flyrail/package.json').version,
    loadedFrom: loadedFrom(),
    bundleId: bundle.id,
    bundleVersion: bundle.version,
    install: installed.map((result) => result.status),
    inspect: inspected.map((inspection) => inspection.observation.isCurrent),
    concurrent: concurrent.map((result) => result.status),
    abort: {
      lockName,
      firstSync: first.status,
      ...aborted,
      afterRelease: afterRelease.status,
      removed: removed.status,
    },
    uninstall: uninstalled.map((result) => result.status),
    skillRemoved: !(await exists(skillFile)),
  };
}
