import { describe, expect, test } from 'vitest';
import { Target, ValueError } from 'flyrail';

describe('targets', () => {
  test('a codex project target roots skills under .agents/skills', () => {
    const target = Target.project('codex', '/work/app');
    expect(target.root).toBe('/work/app/.agents/skills');
    expect(target.agent).toBe('codex');
    expect(target.scope).toBe('project');
    expect(target.home).toBeNull();
  });

  test('a directory target keeps its root and has no agent', () => {
    const target = Target.directory('/work/skills');
    expect(target.root).toBe('/work/skills');
    expect(target.agent).toBeNull();
    expect(target.scope).toBe('directory');
  });

  test('a user target honours the relocation variable from its environment', () => {
    const target = Target.user('claude', '/home/u', { CLAUDE_CONFIG_DIR: '/cfg' });
    expect(target.root.startsWith('/cfg')).toBe(true);
    expect(target.home).toBe('/home/u');
    expect(target.environment).toEqual({ CLAUDE_CONFIG_DIR: '/cfg' });
  });

  test('a user target without an environment roots under its home', () => {
    const target = Target.user('claude', '/home/u');
    expect(target.root).toBe('/home/u/.claude/skills');
    expect(target.environment).toEqual({});
  });

  test('a relative home is refused', () => {
    expect(() => Target.user('claude', 'rel')).toThrow(ValueError);
    expect(() => Target.user('claude', 'rel')).toThrow('home must be an absolute path');
  });

  test('a relative project root is refused', () => {
    expect(() => Target.project('codex', 'rel')).toThrow('target root must be an absolute path');
  });

  test('a filesystem root cannot be a skill container', () => {
    expect(() => Target.directory('/')).toThrow(ValueError);
    expect(() => Target.directory('/')).toThrow('a filesystem root cannot be a skill container');
  });

  test('an unknown agent fails with the crate text', () => {
    expect(() => Target.project('nobody' as 'codex', '/work')).toThrow(
      "'nobody' is not a valid Agent",
    );
  });

  test('equals compares by value', () => {
    expect(Target.project('codex', '/a').equals(Target.project('codex', '/a'))).toBe(true);
    expect(Target.project('codex', '/a').equals(Target.project('codex', '/b'))).toBe(false);
    expect(Target.project('codex', '/a').equals(Target.project('claude', '/a'))).toBe(false);
    expect(Target.project('codex', '/a').equals({} as Target)).toBe(false);
  });
});
