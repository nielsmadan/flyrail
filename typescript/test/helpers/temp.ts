import { chmod, mkdir, mkdtemp, rm } from 'node:fs/promises';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { onTestFinished } from 'vitest';

const base = fileURLToPath(new URL('../../../.cache/typescript/tmp/', import.meta.url));

export async function tempRoot(): Promise<string> {
  await mkdir(base, { recursive: true });
  const root = await mkdtemp(join(base, 'test-'));
  await chmod(root, 0o700);
  onTestFinished(() => rm(root, { recursive: true, force: true }));
  return root;
}
