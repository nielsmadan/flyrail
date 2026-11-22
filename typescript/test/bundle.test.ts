import { mkdir, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { describe, expect, test } from 'vitest';
import {
  Bundle,
  BundleError,
  BundleIdentity,
  ObjectValue,
  SkillSpec,
  ValueError,
  directoryEntry,
  fileEntry,
} from 'flyrail';
import { storedZip } from './helpers/zip.js';
import { tempRoot } from './helpers/temp.js';
import { encode } from './helpers/bytes.js';

const manifest = {
  schema_version: 1,
  id: 'team',
  version: '1',
  skills: [{ name: 'review', path: 'skills/review', executables: ['run.sh'] }],
};
const manifestText = JSON.stringify(manifest);
const skillText = '# Review\n';
const scriptText = '#!/bin/sh\n';

async function writeBundle(root: string): Promise<void> {
  await mkdir(join(root, 'skills/review'), { recursive: true });
  await writeFile(join(root, 'flyrail.json'), manifestText);
  await writeFile(join(root, 'skills/review/SKILL.md'), skillText);
  await writeFile(join(root, 'skills/review/run.sh'), scriptText);
}

const memoryEntries = [
  directoryEntry('skills/review'),
  fileEntry('skills/review/SKILL.md', encode(skillText)),
  fileEntry('skills/review/run.sh', encode(scriptText)),
];

describe('Bundle.fromDirectory', () => {
  test('loads identity, skills, entries and a digest', async () => {
    const root = await tempRoot();
    await writeBundle(root);
    const bundle = await Bundle.fromDirectory(root);
    expect(bundle.id).toBe('team');
    expect(bundle.version).toBe('1');
    expect(bundle.identity.equals(new BundleIdentity('team', '1'))).toBe(true);
    expect(bundle.skills.map(({ name, path, executables }) => [name, path, executables])).toEqual([
      ['review', 'skills/review', ['run.sh']],
    ]);
    expect(bundle.entries.map(({ path, kind, executable }) => [path, kind, executable])).toEqual([
      ['review', 'directory', false],
      ['review/SKILL.md', 'file', false],
      ['review/run.sh', 'file', true],
    ]);
    expect(bundle.contentDigest).toMatch(/^[0-9a-f]{64}$/);
    expect(bundle.schemaVersion).toBe(1);
    expect(bundle.sourceRoots).toHaveLength(1);
  });

  test('rejects a relative path before any native call', async () => {
    await expect(Bundle.fromDirectory('relative/dir')).rejects.toThrow(
      new ValueError('bundle root must be an absolute path'),
    );
  });

  test('rejects a missing directory with the io error and errno', async () => {
    const root = await tempRoot();
    const failure: unknown = await Bundle.fromDirectory(join(root, 'absent')).catch(
      (error: unknown) => error,
    );
    expect(failure).toBeInstanceOf(BundleError);
    expect(failure).toMatchObject({ kind: 'io', errno: 2 });
  });

  test('an already aborted signal rejects with its reason', async () => {
    const root = await tempRoot();
    const reason = new Error('stop');
    await expect(Bundle.fromDirectory(root, { signal: AbortSignal.abort(reason) })).rejects.toBe(
      reason,
    );
  });
});

describe('Bundle.fromZip', () => {
  test('loads the same digest as the directory form', async () => {
    const root = await tempRoot();
    await writeBundle(join(root, 'dir'));
    const archive = join(root, 'package.zip');
    await writeFile(
      archive,
      storedZip([
        { name: 'flyrail/flyrail.json', data: encode(manifestText) },
        { name: 'flyrail/skills/review', data: null },
        { name: 'flyrail/skills/review/SKILL.md', data: encode(skillText) },
        { name: 'flyrail/skills/review/run.sh', data: encode(scriptText) },
      ]),
    );
    const zipped = await Bundle.fromZip(archive, 'flyrail');
    const directory = await Bundle.fromDirectory(join(root, 'dir'));
    expect(zipped.contentDigest).toBe(directory.contentDigest);
    expect(zipped.entries).toEqual(directory.entries);
  });

  test('refuses a member declared over the uncompressed byte limit', async () => {
    const root = await tempRoot();
    const archive = storedZip([{ name: 'flyrail/flyrail.json', data: encode(manifestText) }]);
    const oversized = 64 * 1024 * 1024 + 1;
    archive.writeUInt32LE(oversized, archive.indexOf('PK\x03\x04', 0, 'latin1') + 22);
    archive.writeUInt32LE(oversized, archive.indexOf('PK\x01\x02', 0, 'latin1') + 24);
    const path = join(root, 'bomb.zip');
    await writeFile(path, archive);
    const failure: unknown = await Bundle.fromZip(path, 'flyrail').catch((error: unknown) => error);
    expect(failure).toBeInstanceOf(BundleError);
    expect(failure).toMatchObject({
      kind: 'io',
      message: 'archive member exceeds the uncompressed byte limit',
    });
  });

  test('rejects a relative archive path', async () => {
    await expect(Bundle.fromZip('a.zip', 'flyrail')).rejects.toThrow(
      new ValueError('archive path must be an absolute path'),
    );
  });
});

describe('Bundle.fromMemory', () => {
  test('matches the directory digest', async () => {
    const root = await tempRoot();
    await writeBundle(root);
    const loaded = await Bundle.fromDirectory(root);
    const remembered = Bundle.fromMemory(encode(manifestText), memoryEntries);
    expect(remembered.contentDigest).toBe(loaded.contentDigest);
    expect(remembered.sourceRoots).toEqual([]);
  });

  test('a manifest given as a value loads the same bundle', () => {
    const value = new ObjectValue([
      ['schema_version', 1n],
      ['id', 'team'],
      ['version', '1'],
      [
        'skills',
        [
          new ObjectValue([
            ['name', 'review'],
            ['path', 'skills/review'],
            ['executables', ['run.sh']],
          ]),
        ],
      ],
    ]);
    expect(Bundle.fromMemoryValue(value, memoryEntries).contentDigest).toBe(
      Bundle.fromMemory(encode(manifestText), memoryEntries).contentDigest,
    );
  });

  test('copies entry data', () => {
    const data = encode(skillText);
    const entry = fileEntry('skills/review/SKILL.md', data);
    data.fill(0);
    expect(new TextDecoder().decode(entry.data ?? new Uint8Array())).toBe(skillText);
  });

  test('validates an entry where it is consumed', () => {
    expect(() =>
      Bundle.fromMemory(encode(manifestText), [fileEntry('../escape', encode('x'))]),
    ).toThrow(ValueError);
    expect(() =>
      Bundle.fromMemory(encode(manifestText), [
        { path: 'skills/review', kind: 'directory', data: null, executable: true },
      ]),
    ).toThrow(ValueError);
  });

  test('refuses a manifest that is not a bundle', () => {
    expect(() => Bundle.fromMemory(encode('{}'), [])).toThrow(BundleError);
  });
});

describe('models', () => {
  test('BundleIdentity validates and compares', () => {
    expect(() => new BundleIdentity('Not Valid', '1')).toThrow(ValueError);
    expect(new BundleIdentity('team', '1').equals(new BundleIdentity('team', '2'))).toBe(false);
  });

  test('SkillSpec requires the name to match its directory', () => {
    expect(() => new SkillSpec('review', 'skills/other')).toThrow(
      new ValueError('skill name must match its containing directory'),
    );
    const skill = new SkillSpec('review', 'skills/review', ['run.sh']);
    expect(skill.executables).toEqual(['run.sh']);
  });

  test('entry helpers produce frozen plain entries', () => {
    expect(directoryEntry('a')).toEqual({
      path: 'a',
      kind: 'directory',
      data: null,
      executable: false,
    });
    expect(Object.isFrozen(fileEntry('a/b', new Uint8Array(), { executable: true }))).toBe(true);
  });
});
