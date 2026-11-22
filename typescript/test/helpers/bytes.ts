export const encode = (text: string): Uint8Array => new TextEncoder().encode(text);

export function decode(bytes: Uint8Array): string;
export function decode(bytes: Uint8Array | null): string | null;
export function decode(bytes: Uint8Array | null): string | null {
  return bytes === null ? null : new TextDecoder().decode(bytes);
}

export const fromHex = (text: string): Uint8Array => new Uint8Array(Buffer.from(text, 'hex'));
