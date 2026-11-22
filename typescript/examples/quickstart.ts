import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import {
  Bundle,
  BundleIdentity,
  DirectoryStore,
  InstructionArtifact,
  RenderContext,
  Target,
  applyPreview,
  fileEntry,
  inspect,
  install,
  preview,
  remove,
  render,
  uninstall,
} from 'flyrail';

const encode = (text: string): Uint8Array => new TextEncoder().encode(text);

export async function run(root: string): Promise<void> {
  const store = await DirectoryStore.open(join(root, '.flyrail-state'));

  const manifest = {
    schema_version: 1,
    id: 'team-skills',
    version: '1',
    skills: [{ name: 'review', path: 'review' }],
  };
  const skills = Bundle.fromMemory(encode(JSON.stringify(manifest)), [
    fileEntry('review/SKILL.md', encode('# Review\n\nReview the change for correctness.\n')),
  ]);
  const targets = [Target.directory(join(root, 'skills'))];
  const [installed] = await install(skills, targets, store, { lockTimeoutMs: 5000 });
  console.log('install:', installed?.status);
  const [inspection] = await inspect(skills, targets, store);
  console.log('current:', inspection?.observation.isCurrent);
  const [uninstalled] = await uninstall(skills.id, targets, store, { lockTimeoutMs: 5000 });
  console.log('uninstall:', uninstalled?.status);

  const guide = Bundle.fromArtifacts(new BundleIdentity('project-guide', '1'), [
    new InstructionArtifact('guide', 'Run the project checks before committing.\n'),
  ]);
  const context = new RenderContext(Target.project('codex', root), 'linux');
  const rendered = render(guide, context);
  const target = context.installation(store);
  const proposal = await preview(guide, rendered, target);
  const applied = await applyPreview(proposal, { lockTimeoutMs: 5000 });
  console.log('apply:', applied.status);
  console.log(await readFile(join(root, 'AGENTS.md'), 'utf8'));
  const removed = await remove(guide.id, target, { lockTimeoutMs: 5000 });
  console.log('remove:', removed.status);
}
