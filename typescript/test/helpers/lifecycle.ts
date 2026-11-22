import { mkdir } from 'node:fs/promises';
import { join } from 'node:path';
import {
  Bundle,
  BundleIdentity,
  DirectoryStore,
  InstructionArtifact,
  RenderContext,
  Target,
  render,
} from 'flyrail';
import type { InstallationTarget, RenderedBundle, StateStore } from 'flyrail';
import { awaitNative } from '../../src/internal/call.js';
import type { NativeLockHold } from '../../src/native/binding.js';
import { native } from '../../src/native/loader.js';
import { storeHandle } from '../../src/stores.js';

export interface Project {
  readonly root: string;
  readonly bundle: Bundle;
  readonly rendered: RenderedBundle;
  readonly target: InstallationTarget;
}

export const instructionBundle = (id: string, text = 'Run the project checks.\n'): Bundle =>
  Bundle.fromArtifacts(new BundleIdentity(id, 'autumn'), [new InstructionArtifact('guide', text)]);

export async function codexProject(
  root: string,
  store: DirectoryStore,
  id = 'project-guide',
): Promise<Project> {
  await mkdir(root, { recursive: true, mode: 0o700 });
  const bundle = instructionBundle(id);
  const context = new RenderContext(Target.project('codex', root), 'linux');
  return { root, bundle, rendered: render(bundle, context), target: context.installation(store) };
}

export const storeUnder = (root: string): Promise<DirectoryStore> =>
  DirectoryStore.open(join(root, '.cache', 'state'));

export const holdLock = (store: StateStore, name: string): Promise<NativeLockHold> =>
  awaitNative(native.internalHoldLock(storeHandle(store), name));
