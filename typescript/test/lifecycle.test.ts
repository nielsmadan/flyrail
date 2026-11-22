import { readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { describe, expect, test } from 'vitest';
import {
  FlyrailError,
  InstallationObservation,
  InstallationTarget,
  LifecyclePreview,
  TargetError,
  ValueError,
  applyPreview,
  inspectInstallation,
  preview,
  previewRemoval,
  recoverInstallation,
  remove,
  sync,
} from 'flyrail';
import { codexProject, storeUnder } from './helpers/lifecycle.js';
import { tempRoot } from './helpers/temp.js';

const agentsFile = (root: string): Promise<string> => readFile(join(root, 'AGENTS.md'), 'utf8');

describe('configuration lifecycle', () => {
  test('previews, applies, inspects and removes an instruction bundle', async () => {
    const root = await tempRoot();
    const { bundle, rendered, target } = await codexProject(root, await storeUnder(root));

    const proposal = await preview(bundle, rendered, target);
    expect(proposal).toBeInstanceOf(LifecyclePreview);
    expect(proposal.applicable).toBe(true);
    expect(proposal.bundle).toBe(bundle);
    expect(proposal.target).toBe(target);
    expect(proposal.resources.map((plan) => plan.resource)).toEqual([
      { destination: join(root, 'AGENTS.md'), kind: 'document' },
    ]);

    const applied = await applyPreview(proposal);
    expect(['applied', 'unchanged']).toContain(applied.status);
    expect(applied.succeeded).toBe(true);
    expect(applied.observation.target).toBe(target);
    expect(await agentsFile(root)).toContain('Run the project checks.');

    const observed = await inspectInstallation(bundle.id, target);
    expect(observed).toBeInstanceOf(InstallationObservation);
    expect(observed.matches(bundle, rendered)).toBe(true);
    expect(observed.version).toBe('autumn');
    expect(observed.resources[0]?.claims.map((claim) => claim.artifactId)).toEqual(['guide']);

    const removed = await remove(bundle.id, target);
    expect(removed.status).toBe('applied');
    expect((await inspectInstallation(bundle.id, target)).version).toBeNull();
    expect(await agentsFile(root).catch(() => 'absent')).toBe('absent');
  });

  test('a second sync is unchanged', async () => {
    const root = await tempRoot();
    const { bundle, rendered, target } = await codexProject(root, await storeUnder(root));
    expect((await sync(bundle, rendered, target)).status).toBe('applied');
    const again = await sync(bundle, rendered, target);
    expect(again.status).toBe('unchanged');
    expect(again.succeeded).toBe(true);
  });

  test('a foreign edit inside the owned section blocks sync unless replaced', async () => {
    const root = await tempRoot();
    const { bundle, rendered, target } = await codexProject(root, await storeUnder(root));
    await sync(bundle, rendered, target);
    const installed = await agentsFile(root);
    await writeFile(
      join(root, 'AGENTS.md'),
      installed.replace('Run the project checks.', 'Edited by hand.'),
    );

    const blocked = await sync(bundle, rendered, target);
    expect(blocked.status).toBe('failed');
    expect(blocked.succeeded).toBe(false);
    expect(blocked.error).toBeInstanceOf(TargetError);
    expect(blocked.error).toMatchObject({
      code: 'modified',
      path: join(root, 'AGENTS.md'),
      message: 'owned selection cannot be changed',
    });

    const replaced = await sync(bundle, rendered, target, { replaceModified: true });
    expect(replaced.status).toBe('applied');
    expect(await agentsFile(root)).toContain('Run the project checks.');
  });

  test('recovery with nothing interrupted is unchanged', async () => {
    const root = await tempRoot();
    const { bundle, rendered, target } = await codexProject(root, await storeUnder(root));
    await sync(bundle, rendered, target);
    expect((await recoverInstallation(bundle.id, target)).status).toBe('unchanged');
  });

  test('a removal preview carries no bundle and an empty rendering', async () => {
    const root = await tempRoot();
    const { bundle, rendered, target } = await codexProject(root, await storeUnder(root));
    await sync(bundle, rendered, target);
    const proposal = await previewRemoval(bundle.id, target);
    expect(proposal.bundle).toBeNull();
    expect(proposal.rendered.artifacts).toEqual([]);
    expect(proposal.bundleId).toBe(bundle.id);
  });

  test('invalid arguments reject with ValueError before native work', async () => {
    const root = await tempRoot();
    const { bundle, rendered, target } = await codexProject(root, await storeUnder(root));
    await expect(sync(bundle, rendered, target, { lockTimeoutMs: -1 })).rejects.toThrow(
      new ValueError('lock timeout must be finite and nonnegative'),
    );
    await expect(remove(bundle.id, target, { lockTimeoutMs: Number.NaN })).rejects.toBeInstanceOf(
      ValueError,
    );
    await expect(inspectInstallation('Bad Id', target)).rejects.toBeInstanceOf(ValueError);
  });

  test('an already-aborted signal rejects with its reason', async () => {
    const root = await tempRoot();
    const { bundle, rendered, target } = await codexProject(root, await storeUnder(root));
    const reason = new Error('stop');
    await expect(
      sync(bundle, rendered, target, { signal: AbortSignal.abort(reason) }),
    ).rejects.toBe(reason);
    expect((await inspectInstallation(bundle.id, target)).version).toBeNull();
  });

  test('invalid arguments reject with Flyrail errors instead of throwing or leaking raw errors', async () => {
    const root = await tempRoot();
    const { bundle, rendered, target } = await codexProject(root, await storeUnder(root));
    const bad = (value: unknown): never => value as never;

    await expect(
      sync(bundle, rendered, target, bad({ replaceModified: 'yes' })),
    ).rejects.toBeInstanceOf(FlyrailError);
    await expect(inspectInstallation(bad(5), target)).rejects.toBeInstanceOf(FlyrailError);
    await expect(applyPreview(bad(undefined))).rejects.toBeInstanceOf(ValueError);
    await expect(applyPreview(bad(null))).rejects.toBeInstanceOf(ValueError);
  });

  test('an installation target accepts null contexts', async () => {
    const root = await tempRoot();
    const store = await storeUnder(root);
    const target = new InstallationTarget(store, null, null);
    expect(target.context).toEqual({});
    expect(target.routingContext).toEqual({});
  });
});
