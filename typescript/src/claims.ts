import type { Ownership } from './enums.js';
import { runReadOnly, type CallOptions } from './internal/abort.js';
import { awaitNative, callNative } from './internal/call.js';
import { attachHandle, handleOf } from './internal/handles.js';
import {
  buildModel,
  modelHandle,
  type ClaimSelectorWire,
  type ClaimWire,
} from './internal/models.js';
import { requireAbsolute } from './internal/paths.js';
import { requireText } from './internal/text.js';
import { ValueError } from './errors.js';
import type { NativeAuthority } from './native/binding.js';
import { native } from './native/loader.js';
import {
  SectionBoundaries,
  Selector,
  boundariesWire,
  selectorFromWire,
  selectorWire,
} from './content.js';

export const WHOLE = Object.freeze({ kind: 'whole' } as const);

export type ClaimSelector =
  | { readonly kind: 'whole' }
  | { readonly kind: 'subtree'; readonly path: string }
  | SectionBoundaries
  | Selector;

const selectorToWire = (selector: ClaimSelector): ClaimSelectorWire => {
  switch (selector.kind) {
    case 'whole':
      return { kind: 'whole' };
    case 'subtree':
      return { kind: 'subtree', path: requireText(selector.path, 'subtree selector') };
    case 'section':
      return boundariesWire(selector);
    case 'structured':
      return selectorWire(selector);
    default:
      throw new ValueError('claim selector has an unknown kind');
  }
};

const selectorFromClaimWire = (wire: ClaimSelectorWire): ClaimSelector => {
  switch (wire.kind) {
    case 'whole':
      return WHOLE;
    case 'subtree':
      return Object.freeze({ kind: 'subtree', path: wire.path } as const);
    case 'boundaries':
      return new SectionBoundaries(wire.start, wire.end);
    case 'selector':
      return selectorFromWire(wire);
  }
};

export class ResourceAuthority {
  readonly destination: string;

  private constructor(handle: NativeAuthority) {
    this.destination = callNative(() => handle.destination());
    attachHandle(this, handle);
    Object.freeze(this);
  }

  static create(destination: string, options?: CallOptions): Promise<ResourceAuthority> {
    return runReadOnly(options?.signal, async () => {
      const handle = await awaitNative(
        native.authorityCreate(requireAbsolute(destination, 'resource destination')),
      );
      return new ResourceAuthority(handle);
    });
  }
}

export class Claim {
  readonly kind: Ownership;
  readonly selector: ClaimSelector;

  constructor(kind: Ownership, selector: ClaimSelector) {
    const { handle, snapshot } = buildModel<ClaimWire>({
      kind: 'claim',
      ownership: kind,
      selector: selectorToWire(selector),
    });
    this.kind = snapshot.ownership;
    this.selector = selectorFromClaimWire(snapshot.selector);
    attachHandle(this, handle);
    Object.freeze(this);
  }

  overlaps(other: Claim): boolean {
    return callNative(() => modelHandle(this).claimOverlaps(modelHandle(other)));
  }

  identity(authority: ResourceAuthority): string {
    const handle = handleOf(authority);
    if (!(authority instanceof ResourceAuthority) || handle === undefined) {
      throw new ValueError('authority must be a ResourceAuthority');
    }
    return callNative(() => modelHandle(this).claimIdentity(handle as NativeAuthority));
  }

  equals(other: Claim): boolean {
    return callNative(() => modelHandle(this).claimEquals(modelHandle(other)));
  }
}

export function canonicalDestination(path: string, options?: CallOptions): Promise<string> {
  return runReadOnly(options?.signal, () =>
    awaitNative(native.authorityCanonicalDestination(requireAbsolute(path, 'destination path'))),
  );
}

export function transactionPath(destination: string, transactionId: string): string {
  return callNative(() =>
    native.authorityTransactionPath(
      requireAbsolute(destination, 'resource destination'),
      requireText(transactionId, 'transaction id'),
    ),
  );
}

export const claimWire = (claim: Claim): ClaimWire => ({
  kind: 'claim',
  ownership: claim.kind,
  selector: selectorToWire(claim.selector),
});

export const claimFromWire = (wire: ClaimWire): Claim =>
  new Claim(wire.ownership, selectorFromClaimWire(wire.selector));
