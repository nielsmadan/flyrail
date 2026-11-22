import { expect, test } from 'vitest';
import { AGENTS } from 'flyrail';

test('AGENTS lists the agents in the Rust order', () => {
  expect(AGENTS).toEqual(['claude', 'codex', 'opencode', 'pi', 'cursor', 'copilot', 'droid']);
  expect(Object.isFrozen(AGENTS)).toBe(true);
});
