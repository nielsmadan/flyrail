import { readFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';
import { Readable } from 'node:stream';

const request = JSON.parse(readFileSync(0, 'utf8'));
const runner = await import(pathToFileURL(`${request.runtime}/runner.mts`));
if (request.clockStep) {
  let clock = 0;
  Object.defineProperty(performance, 'now', { value: () => (clock += request.clockStep) });
}
if (request.action === 'parse') {
  try {
    const result = runner.parse(Buffer.from(request.data, 'base64'));
    process.stdout.write(JSON.stringify({
      bytesBase64: Buffer.from(JSON.stringify(result)).toString('base64'),
    }));
  } catch (error) {
    process.stdout.write(JSON.stringify({ error: error.message }));
  }
  process.exit(0);
}
if (request.action === 'typed') {
  try {
    const input = runner.parse(Buffer.from(request.input, 'base64'));
    const handlers = request.responses.map((response, index) => ({
      id: `handler-${index}`,
      event: input.event,
      argv: [
        request.node,
        '-e',
        `process.stdout.write(Buffer.from(${JSON.stringify(response)},'base64'))`,
      ],
      env: {},
      cwd: request.cwd,
      timeout_ms: 1000,
      outcomes: ['continue', 'block', 'ask', 'context', 'modify-input'],
      tools: [],
    }));
    const effects = await runner.run({ host: 'test', handlers }, input);
    process.stdout.write(JSON.stringify({
      inputBase64: Buffer.from(JSON.stringify(input)).toString('base64'),
      responseBase64: request.responses.map(response =>
        Buffer.from(JSON.stringify(runner.parse(Buffer.from(response, 'base64')))).toString('base64')),
      effectsBase64: Buffer.from(JSON.stringify(effects)).toString('base64'),
    }));
  } catch (error) {
    process.stdout.write(JSON.stringify({ error: error.message }));
  }
  process.exit(0);
}
if (request.action === 'run') {
  try {
    const result = await runner.run(request.config, request.input);
    process.stdout.write(JSON.stringify({ result }));
  } catch (error) {
    process.stdout.write(JSON.stringify({ error: error.message }));
  }
  process.exit(0);
}
if (request.action === 'readInput') {
  const stream = request.heldOpen
    ? new Readable({ read() {} })
    : Readable.from([Buffer.alloc(1048577)]);
  try {
    const result = await runner.readInput(stream);
    process.stdout.write(JSON.stringify({ result }));
  } catch (error) {
    process.stdout.write(JSON.stringify({ error: error.message }));
  }
  process.exit(0);
}

const bridge = await import(pathToFileURL(`${request.runtime}/bridge.mts`));
const value = request.case;
const capture = request.capture;
const source = `let s="";process.stdin.on("data",c=>s+=c);process.stdin.on("end",()=>{require("node:fs").writeFileSync(${JSON.stringify(capture)},s);process.stdout.write(${JSON.stringify(JSON.stringify(value.response))})})`;
const config = {
  host: value.host,
  handlers: [{
    id: 'handler',
    event: value.event,
    argv: [request.node, '-e', source],
    env: {},
    cwd: request.cwd,
    timeout_ms: 1000,
    outcomes: [value.response.outcome],
    tools: value.tools ?? [],
  }],
};
let observed;
let instrumentation = {};
if (value.host === 'pi') {
  const handlers = new Map();
  bridge.registerPi(config, { on: (event, callback) => handlers.set(event, callback) });
  const original = value.native_input.input;
  const before = JSON.stringify(value.native_input);
  const result = await handlers.get(value.native_event)(
    value.native_input,
    { cwd: request.cwd, mode: 'print', hasUI: false },
  );
  observed = {
    result: result ?? null,
    sameInput: original === value.native_input.input,
  };
  if (JSON.stringify(value.native_input) !== before) observed.input = value.native_input;
  instrumentation = { registered: [...handlers.keys()] };
} else if (value.host === 'opencode') {
  const hooks = bridge.openCode(config, { directory: request.cwd });
  let blocked = null;
  try {
    if (hooks[value.native_event]) await hooks[value.native_event](value.native_input, value.native_output);
  } catch (error) {
    blocked = error.message;
  }
  observed = {
    output: value.native_output,
    blocked,
  };
  instrumentation = { registered: Object.keys(hooks) };
} else {
  const input = bridge.normalize(value.host, value.event, value.native_input);
  const effects = await runner.run(config, input);
  observed = bridge.nativeOutput(value.host, value.event, effects);
}
process.stdout.write(JSON.stringify({
  observed,
  instrumentation,
  packetBase64: readFileSync(capture).toString('base64'),
}));
