import { relative } from 'node:path';
import { describe, expect, test } from 'vitest';
import {
  Bundle,
  BundleIdentity,
  Command,
  EnvRef,
  McpArtifact,
  MemoryStore,
  discoverLoadout,
  previewLoadout,
  type Agent,
} from 'flyrail';
import { fixture } from '../helpers/fixtures.js';
import { fixtureRoot, substitute, writeFiles } from '../helpers/loadout.js';

interface DiscoveryVector {
  readonly name: string;
  readonly files: Readonly<Record<string, string>>;
  readonly env?: Readonly<Record<string, string>>;
  readonly status: 'absent' | 'available' | 'invalid';
  readonly code?: string;
  readonly profile?: string;
  readonly chain?: readonly string[];
  readonly sources?: readonly string[];
  readonly source_paths?: readonly string[];
  readonly harnesses?: readonly Agent[];
  readonly instruction_path?: string;
  readonly instruction_key?: readonly string[];
}

interface PreviewVector {
  readonly name: string;
  readonly files: Readonly<Record<string, string>>;
  readonly harnesses: readonly Agent[];
  readonly code?: string | null;
  readonly env_reference?: boolean;
}

const document = fixture('loadout.json') as
  | {
      readonly discovery: readonly DiscoveryVector[];
      readonly mcp_previews: readonly PreviewVector[];
    }
  | undefined;

describe('loadout.json discovery', () => {
  test.each(document?.discovery ?? [])('$name', async (vector) => {
    const root = await fixtureRoot();
    await writeFiles(root, vector.files);
    const environment = Object.fromEntries(
      Object.entries(vector.env ?? {}).map(([key, value]) => [key, substitute(value, root)]),
    );
    const found = await discoverLoadout({ home: `${root}/home`, environment });
    expect(found.status).toBe(vector.status);
    if (found.status === 'invalid') {
      expect(found.diagnostic.code).toBe(vector.code);
    }
    if (found.status !== 'available') {
      return;
    }
    const { setup } = found;
    expect(setup.profile).toBe(vector.profile);
    expect(setup.profileChain.map((path) => relative(root, path))).toEqual(vector.chain);
    expect(setup.sources.map((source) => source.name)).toEqual(vector.sources);
    expect(setup.harnesses.map((harness) => harness.agent)).toEqual(vector.harnesses);
    if (vector.source_paths !== undefined) {
      expect(setup.sources.map((source) => relative(root, source.path))).toEqual(
        vector.source_paths,
      );
    }
    if (vector.instruction_path !== undefined) {
      expect(
        setup.selectors.map((selector) => [relative(root, selector.manifest), selector.key]),
      ).toEqual([[vector.instruction_path, vector.instruction_key]]);
    }
  });
});

describe('loadout.json mcp_previews', () => {
  test.each(document?.mcp_previews ?? [])('$name', async (vector) => {
    const root = await fixtureRoot();
    await writeFiles(root, vector.files);
    const found = await discoverLoadout({ home: `${root}/home`, environment: {} });
    if (found.status !== 'available') {
      throw new Error(`discovery was ${found.status}`);
    }
    const destination = found.setup.select('local', vector.harnesses);
    const environment = vector.env_reference === true ? { TOKEN: new EnvRef('REVIEW_TOKEN') } : {};
    const bundle = Bundle.fromArtifacts(new BundleIdentity('toolbox', '1'), [
      new McpArtifact('server', 'review', new Command(['review'], environment)),
    ]);
    const proposal = await previewLoadout(bundle, destination, new MemoryStore(), {
      platform: 'macos',
    });
    expect(proposal.applicable).toBe(vector.code === null || vector.code === undefined);
    if (vector.code !== null && vector.code !== undefined) {
      expect(proposal.diagnostic?.code).toBe(vector.code);
    }
  });
});
