import { ObjectValue, type SemanticValue } from 'flyrail';

export type Json =
  null | boolean | number | string | readonly Json[] | { readonly [key: string]: Json };

export function jsonValue(json: Json): SemanticValue {
  if (json === null || typeof json === 'boolean' || typeof json === 'string') {
    return json;
  }
  if (typeof json === 'number') {
    return Number.isInteger(json) ? BigInt(json) : json;
  }
  if (Array.isArray(json)) {
    return (json as readonly Json[]).map(jsonValue);
  }
  return new ObjectValue(
    Object.entries(json as { readonly [key: string]: Json }).map(
      ([key, member]) => [key, jsonValue(member)] as const,
    ),
  );
}
