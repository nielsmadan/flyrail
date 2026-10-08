import { spawn } from 'node:child_process';
import { Readable } from 'node:stream';
import { finished } from 'node:stream/promises';
import type { ReadableStream } from 'node:stream/web';

type Child = { pid: number | undefined; stdout: Readable; stderr: Readable; exited: Promise<void>; completed: Promise<number | null>; kill(): void; closeInput(): void };
type BunProcess = { pid: number; stdout: ReadableStream<Uint8Array>; stderr: ReadableStream<Uint8Array>; exited: Promise<number>; kill(signal: 'SIGKILL'): void };
type BunRuntime = { spawn(options: { cmd: string[]; cwd: string; env: NodeJS.ProcessEnv; stdin: Uint8Array; stdout: 'pipe'; stderr: 'pipe'; detached: boolean; windowsHide: boolean }): BunProcess };

export function start(argv: string[], cwd: string, env: NodeJS.ProcessEnv, input: Uint8Array): Child {
  const bun = (globalThis as typeof globalThis & { Bun?: BunRuntime }).Bun;
  if (bun) {
    // Bun 1.4.2's node:child_process env copy drops __proto__.
    const child = bun.spawn({ cmd: argv, cwd, env, stdin: input, stdout: 'pipe', stderr: 'pipe', detached: process.platform !== 'win32', windowsHide: true });
    const stdout = Readable.fromWeb(child.stdout);
    const stderr = Readable.fromWeb(child.stderr);
    return {
      pid: child.pid, stdout, stderr,
      exited: child.exited.then(() => {}),
      completed: Promise.all([child.exited, finished(stdout), finished(stderr)]).then(([code]) => code),
      kill: () => child.kill('SIGKILL'),
      closeInput: () => {},
    };
  }
  const child = spawn(argv[0], argv.slice(1), { cwd, env, shell: false, detached: process.platform !== 'win32', stdio: ['pipe', 'pipe', 'pipe'], windowsHide: true });
  const exited = new Promise<void>(resolve => child.once('exit', () => resolve()));
  const completed = new Promise<number | null>((resolve, reject) => {
    child.once('error', reject);
    child.once('close', resolve);
  });
  child.stdin.on('error', () => {});
  child.stdin.end(input);
  return {
    pid: child.pid, stdout: child.stdout, stderr: child.stderr, exited, completed,
    kill: () => { child.kill('SIGKILL'); },
    closeInput: () => { child.stdin.destroy(); },
  };
}
