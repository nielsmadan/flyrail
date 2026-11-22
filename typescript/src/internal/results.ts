import { TargetError } from '../errors.js';
import type { TargetErrorWire } from './models.js';

export const targetErrorFromWire = (wire: TargetErrorWire | null): TargetError | null =>
  wire === null ? null : new TargetError(wire.code, wire.message, wire.path, wire.errno);

export const frozenPaths = (paths: readonly string[]): readonly string[] =>
  Object.freeze([...paths]);
