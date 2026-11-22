import { accessSync, constants } from 'node:fs';
import { delimiter, join } from 'node:path';
import { fileURLToPath } from 'node:url';

export type Tool = 'node' | 'bun' | 'python';

const executable = (path: string): boolean => {
  try {
    accessSync(path, constants.X_OK);
    return true;
  } catch {
    return false;
  }
};

const onPath = (name: string): string | null =>
  (process.env.PATH ?? '')
    .split(delimiter)
    .filter((entry) => entry !== '')
    .map((entry) => join(entry, name))
    .find(executable) ?? null;

const pinnedBun = fileURLToPath(new URL('../../../.cache/tools/bun-1.4.2/bun', import.meta.url));

const python = fileURLToPath(new URL('../../../python/.venv/bin/python', import.meta.url));

export function locateTool(name: Tool): string | null {
  switch (name) {
    case 'node':
      return process.execPath;
    case 'bun':
      return executable(pinnedBun) ? pinnedBun : onPath('bun');
    case 'python':
      return executable(python) ? python : null;
  }
}

export function requireTool(name: Tool): string {
  const path = locateTool(name);
  if (path === null) {
    throw new Error(`${name} is required but was not found`);
  }
  return path;
}
