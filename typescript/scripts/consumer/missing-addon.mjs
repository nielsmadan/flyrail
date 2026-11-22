import { NativeLoadError } from 'flyrail/errors';

const failure = await import('flyrail').then(
  () => null,
  (error) => error,
);
console.log(
  JSON.stringify({
    loaded: failure === null,
    nativeLoadError: failure instanceof NativeLoadError,
    name: failure?.name ?? null,
    attempts: failure?.attempts?.length ?? null,
  }),
);
