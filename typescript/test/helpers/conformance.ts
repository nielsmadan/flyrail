import { callNative } from '../../src/internal/call.js';
import { native } from '../../src/native/loader.js';

export type StateRecordType = 'Index' | 'Journal' | 'Preparation' | 'Receipt' | 'ResourceRef';

export const canonicalState = (type: StateRecordType, bytes: Uint8Array): Uint8Array =>
  callNative(() => native.internalCanonicalState(type, bytes));
