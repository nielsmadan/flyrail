import { pathToFileURL } from 'node:url';
import { readFileSync } from 'node:fs';
const request = JSON.parse(readFileSync(0, 'utf8'));
const { run, parse } = await import(pathToFileURL(request.runtime));
if (request.clockStep) { let clock = 0; Object.defineProperty(performance, 'now', { value: () => (clock += request.clockStep) }); }
try {
  const result = request.parse === undefined ? await run(request.config, request.input) : parse(Buffer.from(request.parse, 'base64'));
  process.stdout.write(JSON.stringify({ result }));
} catch (error) { process.stdout.write(JSON.stringify({ error: error.message })); }
