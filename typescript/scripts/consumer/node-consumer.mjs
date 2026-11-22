import { resolve } from 'node:path';
import { runConsumer } from './core.mjs';

const runtime =
  process.versions.bun !== undefined
    ? 'bun'
    : process.versions.electron !== undefined
      ? 'electron-run-as-node'
      : 'node';
const scratch = process.argv.at(-1);
if (scratch === undefined || process.argv.length < 3) {
  console.error('usage: node-consumer.mjs <scratch>');
  process.exit(2);
}
console.log(JSON.stringify(await runConsumer(resolve(scratch), runtime)));
