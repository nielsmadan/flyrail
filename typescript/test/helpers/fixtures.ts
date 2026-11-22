import { existsSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const fixtures = new URL('../../../spec/fixtures/', import.meta.url);

export const fixturesRequired = (): boolean => process.env.FLYRAIL_REQUIRE_FIXTURES === '1';

export function fixture(name: string): unknown {
  const path = fileURLToPath(new URL(name, fixtures));
  if (existsSync(path)) {
    return JSON.parse(readFileSync(path, 'utf8')) as unknown;
  }
  if (fixturesRequired()) {
    throw new Error(`required fixture is missing: ${path}`);
  }
  console.warn(`skipping: fixture ${name} is absent`);
  return undefined;
}
