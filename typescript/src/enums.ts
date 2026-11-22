import { native } from './native/loader.js';

export type Agent = 'claude' | 'codex' | 'opencode' | 'pi' | 'cursor' | 'copilot' | 'droid';
export const AGENTS: readonly Agent[] = Object.freeze(native.targetsAgents());
export type TargetScope = 'directory' | 'user' | 'project';
export type Surface = 'cli' | 'ide' | 'vscode';
export type Platform = 'macos' | 'linux' | 'windows';
export type Family = 'skills' | 'instructions' | 'mcp' | 'hooks';
export type HookEvent = 'session-start' | 'before-tool' | 'after-tool' | 'prompt' | 'stop';
export type HookOutcome = 'continue' | 'block' | 'ask' | 'modify-input' | 'context';
export type HookShell = 'posix' | 'cmd';
export type DocumentFormat = 'json' | 'jsonc' | 'toml';
export type FileMode = 'private' | 'readable' | 'executable';
export type Position = 'first' | 'last' | 'before' | 'after';
export type Ownership = 'file' | 'tree' | 'subtree' | 'section' | 'structured';
export type Acquisition = 'conflict' | 'takeover' | 'adopt';
export type Disposition = 'created' | 'taken-over' | 'adopted';
export type EditStatus =
  | 'created'
  | 'taken-over'
  | 'adopted'
  | 'current'
  | 'updated'
  | 'removed'
  | 'modified'
  | 'conflict';
export type NoticeKind = 'prerequisite' | 'activation' | 'unsupported';
export type DependencyMode = 'revision' | 'stable-reference';
export type ResourceKind = 'document' | 'tree' | 'skill-container';
export type OperationStatus = 'applied' | 'unchanged' | 'failed' | 'partial' | 'incomplete';
export type ErrorCode =
  | 'busy'
  | 'conflict'
  | 'modified'
  | 'update_required'
  | 'unsupported'
  | 'io_error'
  | 'unsafe_path'
  | 'invalid_state'
  | 'concurrent_change'
  | 'recovery_needed';
export type ObservationState = 'absent' | 'installed' | 'recovery_needed' | 'unknown';
export type ModificationKind =
  'missing' | 'added' | 'type_changed' | 'content_changed' | 'executable_changed';
export type ChangeAction = 'create' | 'update' | 'delete' | 'unchanged' | 'conflict';
