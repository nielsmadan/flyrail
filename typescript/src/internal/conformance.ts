import { native } from '../native/loader.js';
import { callNative } from './call.js';

export type StateRecordType = 'Index' | 'Journal' | 'Preparation' | 'Receipt' | 'ResourceRef';

export const canonicalState = (type: StateRecordType, bytes: Uint8Array): Uint8Array =>
  callNative(() => native.internalCanonicalState(type, bytes));
