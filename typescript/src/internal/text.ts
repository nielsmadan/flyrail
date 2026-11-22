import { ValueError } from '../errors.js';

const CHUNK = 8192;

export function requireText(text: string, label: string): string {
  if (typeof text !== 'string') {
    throw new ValueError(`${label} must be a string`);
  }
  if (!text.isWellFormed()) {
    throw new ValueError(`${label} must be well-formed Unicode text`);
  }
  return text;
}

export const requireBundleId = (id: string): string => requireText(id, 'bundle id');

export function toUnits(text: string): Uint16Array {
  if (typeof text !== 'string') {
    throw new ValueError('hook text must be a string');
  }
  return Uint16Array.from({ length: text.length }, (_, index) => text.charCodeAt(index));
}

export function fromUnits(units: Uint16Array): string {
  return Array.from({ length: Math.ceil(units.length / CHUNK) }, (_, index) =>
    String.fromCharCode(...units.subarray(index * CHUNK, (index + 1) * CHUNK)),
  ).join('');
}
