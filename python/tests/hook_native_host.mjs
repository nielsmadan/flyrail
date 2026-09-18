import assert from 'node:assert/strict';
import { existsSync, readFileSync, rmSync } from 'node:fs';
import { pathToFileURL } from 'node:url';

const request = JSON.parse(readFileSync(0, 'utf8'));
let callback;
if (request.host === 'pi') {
  const { createJiti } = await import(pathToFileURL(process.env.FLYRAIL_HOOK_TOOLS + '/node_modules/jiti/lib/jiti-static.mjs'));
  const factory = await createJiti(import.meta.url, { moduleCache: false, fsCache: false, tryNative: false }).import(request.entry, { default: true });
  await factory({ on: (event, handler) => { assert.equal(event, 'tool_result'); callback = handler; } });
} else {
  const { default: factory } = await import(pathToFileURL(request.entry));
  callback = (await factory({ directory: request.cwd }))['tool.execute.after'];
}
assert.equal(typeof callback, 'function');
const cyclic = {};
cyclic.self = cyclic;
const cases = [
  ['nan', NaN], ['infinity', Infinity], ['negative-infinity', -Infinity],
  ['undefined', undefined], ['function', () => 1], ['bigint', 1n],
  ['symbol', Symbol('metric')], ['date', new Date(0)], ['cycle', cyclic],
  ['finite', 4.5], ['omitted', undefined],
];
const observations = [];
for (const [name, metric] of cases) {
  rmSync(request.capture, { force: true });
  const valid = name === 'finite' || name === 'omitted';
  const details = name === 'omitted' ? undefined : { nested: [metric] };
  if (request.host === 'pi') {
    const raw = { toolName: 'custom', input: {}, content: [{ type: 'text', text: 'original' }], isError: false, ...(details === undefined ? {} : { details }) };
    const before = { ...raw, input: {}, content: [{ type: 'text', text: 'original' }] };
    const response = await callback(raw, { cwd: request.cwd, mode: 'print', hasUI: false });
    assert.deepEqual(raw, before);
    assert.deepEqual(response, valid ? { content: [...before.content, { type: 'text', text: 'native context' }] } : undefined);
  } else {
    const output = { title: 'original title', output: 'original', ...(details === undefined ? {} : { metadata: details }) };
    const before = { ...output };
    await callback({ tool: 'custom', sessionID: 's', callID: 'c', args: {} }, output);
    assert.deepEqual(output, valid ? { ...before, output: 'original\nnative context' } : before);
  }
  assert.equal(existsSync(request.capture), valid);
  const packet = valid ? JSON.parse(readFileSync(request.capture, 'utf8')) : null;
  const selected = packet?.result[request.host === 'pi' ? 'details' : 'metadata'];
  if (name === 'finite') assert.deepEqual(selected, { nested: [4.5] });
  if (name === 'omitted') assert.equal(selected, undefined);
  observations.push({ name, executed: valid });
}
process.stdout.write(JSON.stringify(observations));
