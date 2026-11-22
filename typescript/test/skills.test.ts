import { readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { describe, expect, test } from 'vitest';
import {
  Bundle,
  BundleIdentity,
  InstructionArtifact,
  MemoryStore,
  RenderedBundle,
  Target,
  ValueError,
  inspect,
  install,
  renderSkills,
  uninstall,
  update,
} from 'flyrail';
import { skillBundle } from './helpers/skills.js';
import { tempRoot } from './helpers/temp.js';
import { exists } from './helpers/files.js';

describe('skill lifecycle', () => {
  test('installs a two-skill bundle into two targets', async () => {
    const root = await tempRoot();
    const targets = [Target.directory(join(root, 'one')), Target.directory(join(root, 'two'))];
    const results = await install(skillBundle(), targets, new MemoryStore());
    expect(results.map((result) => result.status)).toEqual(['applied', 'applied']);
    expect(results.map((result) => result.succeeded)).toEqual([true, true]);
    expect(results.map((result) => result.observation.isCurrent)).toEqual([true, true]);
    expect(results[0]?.target).toBe(targets[0]);
    expect(results[1]?.target).toBe(targets[1]);
    expect(results.every((result) => Object.isFrozen(result))).toBe(true);
    expect(await readFile(join(root, 'one', 'review', 'SKILL.md'), 'utf8')).toBe('# Review 1\n');
    expect(await exists(join(root, 'two', 'deploy', 'SKILL.md'))).toBe(true);
    expect(results[0]?.observation.installed?.entries.map((entry) => entry.path)).toContain(
      'review/SKILL.md',
    );
    expect(results[0]?.observation.installed?.entries.find((entry) => !entry.directory)?.size).toBe(
      BigInt('# Review 1\n'.length),
    );
  });

  test('a changed version needs update, and update applies it', async () => {
    const root = await tempRoot();
    const targets = [Target.directory(join(root, 'skills'))];
    const store = new MemoryStore();
    await install(skillBundle('1'), targets, store);
    const blocked = await install(skillBundle('2'), targets, store);
    expect(blocked[0]?.status).toBe('failed');
    expect(blocked[0]?.error?.code).toBe('update_required');
    const updated = await update(skillBundle('2'), targets, store);
    expect(updated[0]?.status).toBe('applied');
    expect(await readFile(join(root, 'skills', 'review', 'SKILL.md'), 'utf8')).toBe('# Review 2\n');
  });

  test('a lock timeout longer than the native duration range is a very long wait', async () => {
    const root = await tempRoot();
    const targets = [Target.directory(join(root, 'skills'))];
    const results = await install(skillBundle(), targets, new MemoryStore(), {
      lockTimeoutMs: Number.MAX_VALUE,
    });
    expect(results.map((result) => result.status)).toEqual(['applied']);
  });

  test('a non-string path is a ValueError', () => {
    expect(() => Target.directory(123 as unknown as string)).toThrow(
      new ValueError('target root must be a string'),
    );
  });

  test('the same root twice is an alias of the first', async () => {
    const root = await tempRoot();
    const targets = [
      Target.directory(join(root, 'skills')),
      Target.directory(join(root, 'skills')),
    ];
    const results = await install(skillBundle(), targets, new MemoryStore());
    expect(results.map((result) => result.aliasOf)).toEqual([null, 0]);
  });

  test('uninstall removes the skill directories', async () => {
    const root = await tempRoot();
    const targets = [Target.directory(join(root, 'skills'))];
    const store = new MemoryStore();
    await install(skillBundle(), targets, store);
    const removed = await uninstall('team', targets, store);
    expect(removed[0]?.status).toBe('applied');
    expect(await exists(join(root, 'skills', 'review'))).toBe(false);
    expect(await exists(join(root, 'skills', 'deploy'))).toBe(false);
  });

  test('inspect reports a manual edit as a content_changed modification', async () => {
    const root = await tempRoot();
    const targets = [Target.directory(join(root, 'skills'))];
    const store = new MemoryStore();
    const bundle = skillBundle();
    await install(bundle, targets, store);
    await writeFile(join(root, 'skills', 'review', 'SKILL.md'), 'edited\n');
    const [inspection] = await inspect(bundle, targets, store);
    expect(inspection?.target).toBe(targets[0]);
    expect(inspection?.observation.isCurrent).toBe(false);
    expect(inspection?.observation.modifications).toEqual([
      { bundleId: 'team', path: 'review', kind: 'content_changed' },
    ]);
  });

  test('update replaces a modified skill only with replaceModified', async () => {
    const root = await tempRoot();
    const targets = [Target.directory(join(root, 'skills'))];
    const store = new MemoryStore();
    const bundle = skillBundle();
    const file = join(root, 'skills', 'review', 'SKILL.md');
    await install(bundle, targets, store);
    await writeFile(file, 'edited\n');
    const blocked = await update(bundle, targets, store);
    expect(blocked[0]?.status).toBe('failed');
    expect(await readFile(file, 'utf8')).toBe('edited\n');
    const replaced = await update(bundle, targets, store, { replaceModified: true });
    expect(replaced[0]?.status).toBe('applied');
    expect(await readFile(file, 'utf8')).toBe('# Review 1\n');
  });

  test('uninstall removes a modified skill only with replaceModified', async () => {
    const root = await tempRoot();
    const targets = [Target.directory(join(root, 'skills'))];
    const store = new MemoryStore();
    const file = join(root, 'skills', 'review', 'SKILL.md');
    await install(skillBundle(), targets, store);
    await writeFile(file, 'edited\n');
    const blocked = await uninstall('team', targets, store);
    expect(blocked[0]?.status).toBe('failed');
    expect(await exists(file)).toBe(true);
    const removed = await uninstall('team', targets, store, { replaceModified: true });
    expect(removed[0]?.status).toBe('applied');
    expect(await exists(file)).toBe(false);
  });

  test('inspect and renderSkills reject with the reason of an aborted signal', async () => {
    const root = await tempRoot();
    const target = Target.directory(join(root, 'skills'));
    const controller = new AbortController();
    const reason = new Error('stop');
    controller.abort(reason);
    await expect(
      inspect(skillBundle(), [target], new MemoryStore(), { signal: controller.signal }),
    ).rejects.toBe(reason);
    await expect(renderSkills(skillBundle(), target, { signal: controller.signal })).rejects.toBe(
      reason,
    );
  });

  test('update and uninstall reject an invalid lock timeout and an aborted signal', async () => {
    const root = await tempRoot();
    const targets = [Target.directory(join(root, 'skills'))];
    const store = new MemoryStore();
    const controller = new AbortController();
    const reason = new Error('stop');
    controller.abort(reason);
    await expect(
      update(skillBundle(), targets, store, { lockTimeoutMs: -1 }),
    ).rejects.toBeInstanceOf(ValueError);
    await expect(uninstall('team', targets, store, { lockTimeoutMs: -1 })).rejects.toBeInstanceOf(
      ValueError,
    );
    await expect(update(skillBundle(), targets, store, { signal: controller.signal })).rejects.toBe(
      reason,
    );
    await expect(uninstall('team', targets, store, { signal: controller.signal })).rejects.toBe(
      reason,
    );
  });

  test('inspect of an untouched target reports absent', async () => {
    const root = await tempRoot();
    const [inspection] = await inspect(
      skillBundle(),
      [Target.directory(join(root, 'skills'))],
      new MemoryStore(),
    );
    expect(inspection?.observation.state).toBe('absent');
    expect(inspection?.observation.installed).toBeNull();
  });

  test('renderSkills renders the skill entries', async () => {
    const root = await tempRoot();
    const rendered = await renderSkills(skillBundle(), Target.directory(join(root, 'skills')));
    expect(rendered).toBeInstanceOf(RenderedBundle);
    expect(rendered.artifacts.map((artifact) => artifact.id).sort()).toEqual(['deploy', 'review']);
  });

  test('a bundle with an instruction artifact is rejected as non-skill', async () => {
    const root = await tempRoot();
    const bundle = Bundle.fromArtifacts(new BundleIdentity('guide', '1'), [
      new InstructionArtifact('guide', 'text'),
    ]);
    await expect(
      install(bundle, [Target.directory(join(root, 'skills'))], new MemoryStore()),
    ).rejects.toBeInstanceOf(ValueError);
  });

  test('an invalid lock timeout rejects with ValueError', async () => {
    const root = await tempRoot();
    await expect(
      install(skillBundle(), [Target.directory(join(root, 'skills'))], new MemoryStore(), {
        lockTimeoutMs: -1,
      }),
    ).rejects.toBeInstanceOf(ValueError);
  });

  test('an already aborted signal rejects with its reason and writes nothing', async () => {
    const root = await tempRoot();
    const controller = new AbortController();
    const reason = new Error('stop');
    controller.abort(reason);
    await expect(
      install(skillBundle(), [Target.directory(join(root, 'skills'))], new MemoryStore(), {
        signal: controller.signal,
      }),
    ).rejects.toBe(reason);
    expect(await exists(join(root, 'skills'))).toBe(false);
  });
});
