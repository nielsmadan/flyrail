import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { describe, expect, test } from 'vitest';
import {
  Bundle,
  BundleIdentity,
  Command,
  FlyrailError,
  InstallationObservation,
  InstructionArtifact,
  LoadoutDestination,
  LoadoutLocation,
  LoadoutPreview,
  LoadoutSetup,
  McpArtifact,
  ValueError,
  applyLoadout,
  discoverLoadout,
  inspectLoadout,
  previewLoadout,
  previewLoadoutRemoval,
  recoverLoadout,
} from 'flyrail';
import { storeUnder } from './helpers/lifecycle.js';
import { fixtureRoot, writeFiles } from './helpers/loadout.js';

const registry = {
  'home/.config/loadout/config.toml': "source='{root}/source'\n",
  'source/loadout.toml':
    "# keep me\n[[source]]\nname='local'\npath='.'\n[claude]\ninstructions=[]\n",
};

const guide = (): Bundle =>
  Bundle.fromArtifacts(new BundleIdentity('loadout-probe', 'autumn'), [
    new InstructionArtifact('guide', 'Read the diff.\n'),
  ]);

const available = async (root: string): Promise<LoadoutSetup> => {
  await writeFiles(root, registry);
  const found = await discoverLoadout({ home: join(root, 'home'), environment: {} });
  if (found.status !== 'available') {
    throw new Error(`discovery was ${found.status}`);
  }
  return found.setup;
};

describe('Global Loadout', () => {
  test('discovery reports an absent registration', async () => {
    const root = await fixtureRoot();
    expect(await discoverLoadout({ home: join(root, 'home'), environment: {} })).toEqual({
      status: 'absent',
      registry: join(root, 'home/.config/loadout/config.toml'),
    });
  });

  test('a configuration directory takes the place of the environment', async () => {
    const root = await fixtureRoot();
    await writeFiles(root, { 'cfg/loadout/config.toml': 'source=1\n' });
    const found = await discoverLoadout({
      home: join(root, 'home'),
      configHome: join(root, 'cfg'),
      environment: { XDG_CONFIG_HOME: join(root, 'ignored') },
    });
    expect(found.status).toBe('invalid');
    expect(found.registry).toBe(join(root, 'cfg/loadout/config.toml'));
  });

  test('installs, inspects and removes a source installation', async () => {
    const root = await fixtureRoot();
    const setup = await available(root);
    const store = await storeUnder(root);
    const bundle = guide();

    expect(setup).toBeInstanceOf(LoadoutSetup);
    expect(setup.profile).toBe('default');
    expect(setup.eligibleSources(bundle, ['claude']).map((source) => source.name)).toEqual([
      'local',
    ]);
    const destination = setup.select('local', ['claude']);
    expect(destination).toBeInstanceOf(LoadoutDestination);
    expect(destination.harnesses).toEqual(['claude']);
    expect(destination.location).toBeInstanceOf(LoadoutLocation);

    const proposal = await previewLoadout(bundle, destination, store, { platform: 'macos' });
    expect(proposal).toBeInstanceOf(LoadoutPreview);
    expect(proposal.applicable).toBe(true);
    expect(proposal.location.sourceName).toBe('local');
    expect(proposal.impact.map((impact) => impact.category)).toContain('instructions');

    const applied = await applyLoadout(proposal);
    expect(applied.syncRequired).toBe(true);
    expect(applied.lifecycle.status).toBe('applied');
    expect(await readFile(join(root, 'source/loadout.toml'), 'utf8')).toContain('# keep me');

    const observed = await inspectLoadout('loadout-probe', destination.location, store);
    expect(observed).toBeInstanceOf(InstallationObservation);
    expect(observed.version).toBe('autumn');

    const removal = await previewLoadoutRemoval('loadout-probe', destination.location, store);
    expect(removal.applicable).toBe(true);
    const removed = await applyLoadout(removal);
    expect(removed.lifecycle.status).toBe('applied');
    expect((await inspectLoadout('loadout-probe', destination.location, store)).version).toBeNull();

    const recovered = await recoverLoadout('loadout-probe', destination.location, store);
    expect(recovered.location).toBe(destination.location);
    expect(recovered.lifecycle.status).toBe('unchanged');
  });

  test('selecting an undeclared source throws Rust text', async () => {
    const setup = await available(await fixtureRoot());
    expect(() => setup.select('missing', ['claude'])).toThrow(ValueError);
    expect(() => setup.select('missing', ['claude'])).toThrow(
      'Select a declared Loadout source, found no source "missing"',
    );
    expect(() => setup.select('local', [])).toThrow(
      'Select one or more distinct Loadout harnesses',
    );
  });

  test('a relative home rejects', async () => {
    await expect(discoverLoadout({ home: 'rel', environment: {} })).rejects.toBeInstanceOf(
      ValueError,
    );
  });

  test('a missing environment rejects', async () => {
    const options = { home: '/' } as unknown as Parameters<typeof discoverLoadout>[0];
    await expect(discoverLoadout(options)).rejects.toThrow('environment must be an object');
  });

  test('accepting the shared scope makes a wider selection applicable', async () => {
    const root = await fixtureRoot();
    await writeFiles(root, {
      'home/.config/loadout/config.toml': "source = '{root}/setup'\n",
      'setup/loadout.toml': "[[source]]\nname = 'local'\npath = '.'\n[claude]\n[codex]\n",
    });
    const found = await discoverLoadout({ home: join(root, 'home'), environment: {} });
    if (found.status !== 'available') {
      throw new Error(`discovery was ${found.status}`);
    }
    const bundle = Bundle.fromArtifacts(new BundleIdentity('toolbox', '1'), [
      new McpArtifact('server', 'review', new Command(['review'])),
    ]);
    const store = await storeUnder(root);
    const refused = await previewLoadout(bundle, found.setup.select('local', ['claude']), store, {
      platform: 'macos',
    });
    expect(refused.diagnostic?.code).toBe('shared-scope');
    const accepted = await previewLoadout(
      bundle,
      found.setup.select('local', ['claude'], { acceptSharedScope: true }),
      store,
      { platform: 'macos' },
    );
    expect(accepted.applicable).toBe(true);
  });

  test('an aborted discovery rejects with the reason', async () => {
    const reason = new Error('stop');
    const controller = new AbortController();
    controller.abort(reason);
    await expect(
      discoverLoadout({ home: '/', environment: {}, signal: controller.signal }),
    ).rejects.toBe(reason);
  });

  test('a location validates its fields', () => {
    expect(() => new LoadoutLocation('rel', 'default', 'local', '/source')).toThrow(ValueError);
    expect(() => new LoadoutLocation('/root', 'default', 'a/b', '/source')).toThrow(
      'Select one Loadout source name',
    );
  });

  test('a diagnostic is a FlyrailError with a code and path', async () => {
    const root = await fixtureRoot();
    await writeFiles(root, { 'home/.config/loadout/config.toml': 'source = [\n' });
    const found = await discoverLoadout({ home: join(root, 'home'), environment: {} });
    if (found.status !== 'invalid') {
      throw new Error(`discovery was ${found.status}`);
    }
    expect(found.diagnostic).toBeInstanceOf(FlyrailError);
    expect(found.diagnostic.code).toBe('invalid-registry');
    expect(found.diagnostic.path).toBe(join(root, 'home/.config/loadout/config.toml'));
  });
});
