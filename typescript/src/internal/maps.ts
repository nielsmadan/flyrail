export function toPairs(
  record: Readonly<Record<string, string>> | null | undefined,
): readonly (readonly [string, string])[] {
  return record === undefined || record === null ? [] : Object.entries(record);
}

export function fromPairs<T>(
  pairs: readonly (readonly [string, T])[],
): Readonly<Record<string, T>> {
  return Object.freeze(Object.fromEntries(pairs));
}

export interface PairObject {
  readonly key: string;
  readonly value: string;
}

export const toPairObjects = (
  record: Readonly<Record<string, string>> | null | undefined,
): PairObject[] => toPairs(record).map(([key, value]) => ({ key, value }));

export const fromPairObjects = (pairs: readonly PairObject[]): Readonly<Record<string, string>> =>
  fromPairs(pairs.map(({ key, value }) => [key, value] as const));
