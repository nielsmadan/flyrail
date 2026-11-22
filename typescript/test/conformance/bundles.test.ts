import { describe, expect, test } from 'vitest';
import { Bundle, directoryEntry, fileEntry, type BundleEntry } from 'flyrail';
import { fixture } from '../helpers/fixtures.js';

interface FixtureEntry {
  readonly path: string;
  readonly data_hex: string | null;
  readonly executable: boolean;
}

interface BundleCase {
  readonly name: string;
  readonly manifest: unknown;
  readonly directories: readonly string[];
  readonly files: readonly { readonly path: string; readonly data_hex: string }[];
  readonly entries: readonly FixtureEntry[];
  readonly content_digest: string;
}

const document = fixture('bundles.json') as { readonly cases: readonly BundleCase[] } | undefined;
const hex = (text: string): Uint8Array => new Uint8Array(Buffer.from(text, 'hex'));

const memoryEntries = (testCase: BundleCase): readonly BundleEntry[] => [
  ...testCase.directories.map((path) => directoryEntry(path)),
  ...testCase.files.map((file) => fileEntry(file.path, hex(file.data_hex))),
];

describe.skipIf(document === undefined)('bundles.json', () => {
  const cases = document?.cases ?? [];

  test('has cases', () => {
    expect(cases.length).toBeGreaterThan(0);
  });

  test.each(cases.map((testCase) => [testCase.name, testCase] as const))(
    '%s loads with the shared digest and entries',
    (_, testCase) => {
      const bundle = Bundle.fromMemory(
        new TextEncoder().encode(JSON.stringify(testCase.manifest)),
        memoryEntries(testCase),
      );
      expect(bundle.contentDigest).toBe(testCase.content_digest);
      expect(
        bundle.entries.map((entry) => ({
          path: entry.path,
          data_hex: entry.data === null ? null : Buffer.from(entry.data).toString('hex'),
          executable: entry.executable,
        })),
      ).toEqual(testCase.entries);
    },
  );
});
