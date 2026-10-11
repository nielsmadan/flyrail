import { pathToFileURL } from 'node:url';
import { readFileSync } from 'node:fs';
const request = JSON.parse(readFileSync(0, 'utf8'));
if (!request.useRealChildTimeout) {
  const realSetTimeout = globalThis.setTimeout;
  const realClearTimeout = globalThis.clearTimeout;
  const disabledChildTimer = Symbol();
  let childTimerPending = true;
  globalThis.setTimeout = (callback, delay, ...args) => {
    if (childTimerPending) {
      childTimerPending = false;
      return disabledChildTimer;
    }
    return realSetTimeout(callback, delay, ...args);
  };
  globalThis.clearTimeout = timer => {
    if (timer !== disabledChildTimer) realClearTimeout(timer);
  };
}
const { run, parse } = await import(pathToFileURL(request.runtime));
if (request.clockStep) { let clock = 0; Object.defineProperty(performance, 'now', { value: () => (clock += request.clockStep) }); }
try {
  const result = request.parse === undefined ? await run(request.config, request.input) : parse(Buffer.from(request.parse, 'base64'));
  if (request.parse === undefined && !request.useRealChildTimeout) await new Promise(resolve => setTimeout(resolve, 0));
  process.stdout.write(JSON.stringify({ result }));
} catch (error) { process.stdout.write(JSON.stringify({ error: error.message })); }
