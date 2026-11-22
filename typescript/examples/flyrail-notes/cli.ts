import { run } from './main.ts';

const controller = new AbortController();
process.once('SIGINT', () => {
  controller.abort(new Error('interrupted'));
});
process.exitCode = await run(process.argv.slice(2), {
  stdout: process.stdout,
  stderr: process.stderr,
  signal: controller.signal,
});
