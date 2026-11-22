import { mkdir, realpath, writeFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { tempRoot } from './temp.js';

export const fixtureRoot = async (): Promise<string> => realpath(await tempRoot());

export const substitute = (text: string, root: string): string => text.replaceAll('{root}', root);

export async function writeFiles(
  root: string,
  files: Readonly<Record<string, string>>,
): Promise<void> {
  for (const [name, text] of Object.entries(files)) {
    const path = join(root, name);
    await mkdir(dirname(path), { recursive: true, mode: 0o700 });
    await writeFile(path, substitute(text, root), { mode: 0o600 });
  }
}
