import { join } from 'node:path';
import { describe, expect, test } from 'vitest';
import { preview, previewRemoval, summarize, sync } from 'flyrail';
import { codexProject, storeUnder } from './helpers/lifecycle.js';
import { tempRoot } from './helpers/temp.js';

describe('summarize', () => {
  test('classifies a first installation as a create', async () => {
    const root = await tempRoot();
    const { bundle, rendered, target } = await codexProject(root, await storeUnder(root));
    const summary = summarize(await preview(bundle, rendered, target));
    expect(summary.agent).toBe('codex');
    expect(summary.scope).toBe('project');
    expect(summary.applicable).toBe(true);
    expect(summary.changes).toEqual([
      {
        artifactIds: ['guide'],
        families: ['instructions'],
        kind: 'document',
        destination: join(root, 'AGENTS.md'),
        action: 'create',
        error: null,
        recoveryPaths: [],
      },
    ]);
  });

  test('a removal preview lists a delete change', async () => {
    const root = await tempRoot();
    const { bundle, rendered, target } = await codexProject(root, await storeUnder(root));
    await sync(bundle, rendered, target);
    const summary = summarize(await previewRemoval(bundle.id, target));
    expect(summary.changes.map((change) => [change.destination, change.action])).toEqual([
      [join(root, 'AGENTS.md'), 'delete'],
    ]);
  });
});
