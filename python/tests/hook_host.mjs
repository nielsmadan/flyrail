import { pathToFileURL } from 'node:url';
import { readFileSync } from 'node:fs';
const request = JSON.parse(readFileSync(0, 'utf8'));
if (request.host === 'pi') {
  const { createJiti } = await import(pathToFileURL(process.env.FLYRAIL_HOOK_TOOLS + '/node_modules/jiti/lib/jiti-static.mjs'));
  const jiti = createJiti(import.meta.url, { moduleCache: false, fsCache: false, tryNative: false });
  const factory = await jiti.import(request.entry, { default: true });
  const handlers = new Map();
  await factory({ on: (event, callback) => handlers.set(event, callback) });
  const original = request.input.input;
  const callback = handlers.get(request.event);
  const result = callback ? await callback(request.input, request.context) : undefined;
  process.stdout.write(JSON.stringify({ input: request.input, result: result ?? null, registered: [...handlers.keys()], sameInput: original === request.input.input }));
} else {
  const { default: factory } = await import(pathToFileURL(request.entry));
  const hooks = await factory({ directory: request.context.cwd });
  let blocked = null;
  try { if (hooks[request.event]) await hooks[request.event](request.input, request.output); }
  catch (error) { blocked = error.message; }
  process.stdout.write(JSON.stringify({ output: request.output, blocked, registered: Object.keys(hooks) }));
}
