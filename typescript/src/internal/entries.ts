import type { BundleEntry } from '../bundle.js';
import type { JsBundleEntry } from '../native/binding.js';
import { requireText } from './text.js';

export const entryToNative = (entry: BundleEntry): JsBundleEntry => ({
  path: requireText(entry.path, 'entry path'),
  kind: entry.kind,
  ...(entry.data === null ? {} : { data: entry.data }),
  executable: entry.executable,
});

export const entryFromNative = (entry: JsBundleEntry): BundleEntry =>
  Object.freeze({
    path: entry.path,
    kind: entry.kind === 'directory' ? 'directory' : 'file',
    data: entry.data === undefined ? null : new Uint8Array(entry.data),
    executable: entry.executable,
  });
