import { describe, expect, test } from 'vitest';
import { canonicalState, type StateRecordType } from '../helpers/conformance.js';
import { fixture } from '../helpers/fixtures.js';
import { encodeRecord, relocated } from '../helpers/state-records.js';
import { tempRoot } from '../helpers/temp.js';

interface NamedRecord {
  readonly name: string;
  readonly record: unknown;
}

interface StateDocument {
  readonly receipts: readonly NamedRecord[];
  readonly journals: readonly NamedRecord[];
  readonly preparations: readonly NamedRecord[];
  readonly transactions: readonly { readonly name: string; readonly journal: string }[];
}

const document = fixture('configurations.json') as StateDocument | undefined;

describe.skipIf(document === undefined)('configurations.json state records', () => {
  const groups: readonly (readonly [string, StateRecordType, readonly NamedRecord[]])[] = [
    ['receipts', 'Receipt', document?.receipts ?? []],
    ['journals', 'Journal', document?.journals ?? []],
    ['preparations', 'Preparation', document?.preparations ?? []],
  ];

  test.each(groups)('%s are present', (_, __, records) => {
    expect(records.length).toBeGreaterThan(0);
  });

  describe.each(groups)('%s', (_, type, records) => {
    test.each(records.map((entry) => [entry.name, entry] as const))(
      '%s canonicalizes to a fixed point',
      async (__, entry) => {
        const root = await tempRoot();
        const first = canonicalState(type, encodeRecord(await relocated(entry.record, root)));
        expect(canonicalState(type, first)).toEqual(first);
      },
    );
  });

  test.each((document?.transactions ?? []).map((vector) => [vector.name, vector] as const))(
    'transaction %s names a journal that canonicalizes',
    async (_, vector) => {
      const journal = document?.journals.find((entry) => entry.name === vector.journal);
      expect(journal).toBeDefined();
      const root = await tempRoot();
      const canonical = canonicalState(
        'Journal',
        encodeRecord(await relocated(journal?.record, root)),
      );
      expect(canonicalState('Journal', canonical)).toEqual(canonical);
    },
  );
});
