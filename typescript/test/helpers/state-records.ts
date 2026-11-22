import { basename, join } from 'node:path';
import {
  Claim,
  Key,
  Member,
  ResourceAuthority,
  SectionBoundaries,
  Selector,
  type Ownership,
} from 'flyrail';

const isPathLeaf = (value: unknown): value is { readonly path: string } =>
  typeof value === 'object' &&
  value !== null &&
  !Array.isArray(value) &&
  Object.keys(value).length === 1 &&
  typeof (value as { path?: unknown }).path === 'string';

interface Tagged {
  readonly type: string;
  readonly fields: Readonly<Record<string, unknown>>;
}

const isTagged = (value: unknown, type: string): value is Tagged =>
  typeof value === 'object' && value !== null && (value as { type?: unknown }).type === type;

const claimOfRecord = (record: Tagged): Claim => {
  const selector = record.fields.selector as Tagged;
  const kind = (record.fields.kind as { readonly value: Ownership }).value;
  if (selector.type === 'SectionBoundaries') {
    return new Claim(
      kind,
      new SectionBoundaries(selector.fields.start as string, selector.fields.end as string),
    );
  }
  const parts = (selector.fields.parts as readonly Tagged[]).map((part) =>
    part.type === 'Key'
      ? new Key(part.fields.name as string)
      : new Member(
          (part.fields.identity as { readonly value: readonly [string, string] }).value[1],
          part.fields.key as readonly string[],
        ),
  );
  return new Claim(kind, new Selector(parts));
};

const withClaimIds = async (receipt: Tagged): Promise<Tagged> => {
  const resource = receipt.fields.resource as Tagged;
  const authority = await ResourceAuthority.create(
    (resource.fields.destination as { readonly path: string }).path,
  );
  const claims = (receipt.fields.claims as readonly Tagged[]).map((owned) => ({
    ...owned,
    fields: {
      ...owned.fields,
      claim_id: claimOfRecord(owned.fields.claim as Tagged).identity(authority),
    },
  }));
  return { ...receipt, fields: { ...receipt.fields, claims } };
};

export const relocated = async (value: unknown, root: string): Promise<unknown> => {
  if (Array.isArray(value)) {
    return Promise.all((value as readonly unknown[]).map((item) => relocated(item, root)));
  }
  if (isPathLeaf(value)) {
    return { path: join(root, basename(value.path)) };
  }
  if (typeof value === 'object' && value !== null) {
    const entries = await Promise.all(
      Object.entries(value).map(async ([key, item]): Promise<[string, unknown]> => [
        key,
        await relocated(item, root),
      ]),
    );
    const moved: unknown = Object.fromEntries(entries);
    return isTagged(moved, 'Receipt') ? withClaimIds(moved) : moved;
  }
  return value;
};

export const encodeRecord = (record: unknown): Uint8Array =>
  new TextEncoder().encode(JSON.stringify(record));
