import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import { NativeLoadError, type NativeLoadAttempt } from '../errors.js';
import type * as Binding from './binding.js';

export type NativeBinding = typeof Binding;

export interface ModuleLoader {
  (id: string): unknown;
  resolve(id: string): string;
}

export interface NativeHost {
  readonly platform: string;
  readonly arch: string;
  readonly load: ModuleLoader;
  readonly devBuild: URL;
  readonly version: string;
}

export interface LoadedNative {
  readonly binding: NativeBinding;
  readonly path: string;
}

const triples: Readonly<Record<string, string>> = {
  'darwin-arm64': 'darwin-arm64',
  'darwin-x64': 'darwin-x64',
  'linux-arm64': 'linux-arm64-gnu',
  'linux-x64': 'linux-x64-gnu',
};

export function nativeTriple(platform: string, arch: string): string {
  const triple = triples[`${platform}-${arch}`];
  if (triple === undefined) {
    throw new NativeLoadError(`flyrail does not support ${platform}-${arch}`);
  }
  return triple;
}

const describe = (error: unknown): string =>
  error instanceof Error ? error.message : String(error);

type Candidate = (load: ModuleLoader) => string;

const attempt = (
  host: NativeHost,
  label: string,
  candidate: Candidate,
): LoadedNative | NativeLoadAttempt => {
  try {
    const path = candidate(host.load);
    const binding = host.load(path) as NativeBinding;
    const found: unknown =
      typeof binding.nativeVersion === 'function' ? binding.nativeVersion() : undefined;
    if (found !== host.version) {
      throw new Error(
        `native addon version ${typeof found === 'string' ? found : 'unknown'} does not match flyrail ${host.version}`,
      );
    }
    return { binding, path };
  } catch (error) {
    return { path: label, message: describe(error) };
  }
};

const isLoaded = (result: LoadedNative | NativeLoadAttempt): result is LoadedNative =>
  'binding' in result;

export function loadNative(host: NativeHost): LoadedNative {
  const triple = nativeTriple(host.platform, host.arch);
  const devPath = fileURLToPath(new URL(`flyrail.${triple}.node`, host.devBuild));
  const platformPackage = `flyrail-${host.platform}-${host.arch}/flyrail.node`;
  const candidates: readonly (readonly [string, Candidate])[] = [
    [devPath, () => devPath],
    [platformPackage, (load) => load.resolve(platformPackage)],
  ];
  const failures: NativeLoadAttempt[] = [];
  for (const [label, candidate] of candidates) {
    const result = attempt(host, label, candidate);
    if (isLoaded(result)) {
      return result;
    }
    failures.push(result);
  }
  const detail = failures.map((failure) => `${failure.path}: ${failure.message}`).join('; ');
  throw new NativeLoadError(`could not load the flyrail native addon (${detail})`, failures);
}

const packageJson = createRequire(import.meta.url)('../../package.json') as { version: string };

const loaded = loadNative({
  platform: process.platform,
  arch: process.arch,
  load: createRequire(import.meta.url),
  devBuild: new URL('../../build/native/', import.meta.url),
  version: packageJson.version,
});

export const native: NativeBinding = loaded.binding;
export const loadedFrom: string = loaded.path;
