import { posix } from 'node:path';
import { ValueError } from '../errors.js';
import { requireText } from './text.js';

export function requireAbsolute(path: string, label: string): string {
  if (!posix.isAbsolute(requireText(path, label))) {
    throw new ValueError(`${label} must be an absolute path`);
  }
  return path;
}

export function optionalAbsolute(path: string | null | undefined, label: string): string | null {
  return path === null || path === undefined ? null : requireAbsolute(path, label);
}
