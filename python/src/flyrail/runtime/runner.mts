import type { Readable } from 'node:stream';
import { start } from './process.mts';

export type Json = null | boolean | number | string | Json[] | { [key: string]: Json };
export type RecordValue = { [key: string]: Json };
export type NativeRecord = Record<string, unknown>;
export type EventName = 'session-start' | 'before-tool' | 'after-tool' | 'prompt' | 'stop';
export type Input = { version: 1; event: EventName; context: NativeRecord; tool?: string; input?: unknown; result?: unknown };
export type Handler = { id: string; event: EventName; argv: string[]; env: Record<string, string | { ref: string }>; cwd: string; timeout_ms: number; outcomes: string[]; tools: string[] };
export type Config = { host: string; handlers: Handler[] };
export type Effects = { input?: Json; contexts: string[]; decision?: 'block' | 'ask'; reason?: string };
const INPUT_LIMIT = 1048576;
const OUTPUT_LIMIT = 65536;
const ERROR_LIMIT = 16384;
export class RuntimeFailure extends Error {
  constructor(code: string) { super(`flyrail-hook:${code}`); }
}
export function object(value: unknown): value is NativeRecord {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}
export function parse(data: Uint8Array): Json {
  let text: string;
  let result: Json;
  try {
    text = new TextDecoder('utf-8', { fatal: true }).decode(data);
    result = JSON.parse(text) as Json;
  } catch { throw new RuntimeFailure('invalid-json'); }
  const stack: { keys: Set<string>; key: boolean; object: boolean }[] = [];
  const tokens = text.matchAll(/"(?:[^"\\]|\\.)*"|[{}\[\],:]|true|false|null|-?(?:0|[1-9]\d*)(?:\.\d+)?(?:[eE][+-]?\d+)?/g);
  for (const [token] of tokens) {
    const current = stack.at(-1);
    if (token === '{' || token === '[') {
      if (stack.length >= 64) throw new RuntimeFailure('json-depth');
      stack.push({ keys: new Set(), key: token === '{', object: token === '{' });
    } else if (token === '}' || token === ']') stack.pop();
    else if (current && token === ',') current.key = current.object;
    else if (current && current.key && token.startsWith('"')) {
      const key = JSON.parse(token) as string;
      if (current.keys.has(key) || ['__proto__', 'constructor', 'prototype'].includes(key)) throw new RuntimeFailure('json-key');
      current.keys.add(key);
      current.key = false;
    } else if (!token.startsWith('"') && ![':', ',', 'true', 'false', 'null'].includes(token) && !Number.isFinite(Number(token))) {
      throw new RuntimeFailure('json-number');
    }
  }
  return result;
}
function failure(error: unknown): RuntimeFailure {
  return error instanceof RuntimeFailure ? error : new RuntimeFailure('runtime');
}
export function report(error: unknown): void { process.stderr.write(`${failure(error).message}\n`); }
function validateNative(value: unknown, depth = 0): void {
  if (value === null || typeof value === 'boolean' || typeof value === 'string') return;
  if (typeof value === 'number') {
    if (!Number.isFinite(value)) throw new RuntimeFailure('json-number');
    return;
  }
  if (typeof value !== 'object') throw new RuntimeFailure('invalid-input');
  if (depth >= 64) throw new RuntimeFailure('json-depth');
  const array = Array.isArray(value);
  const prototype = Object.getPrototypeOf(value);
  if (array ? prototype !== Array.prototype : prototype !== Object.prototype && prototype !== null) throw new RuntimeFailure('invalid-input');
  if (array && Object.keys(value).length !== value.length) throw new RuntimeFailure('invalid-input');
  for (const key of Reflect.ownKeys(value)) {
    if (array && key === 'length') continue;
    const descriptor = Object.getOwnPropertyDescriptor(value, key);
    if (typeof key !== 'string' || !descriptor?.enumerable || !Object.hasOwn(descriptor, 'value')) throw new RuntimeFailure('invalid-input');
    if (array && !/^(0|[1-9]\d*)$/.test(key)) throw new RuntimeFailure('invalid-input');
    validateNative(descriptor.value, depth + 1);
  }
}
function bytes(value: unknown): Buffer {
  let data: Buffer;
  try { validateNative(value); data = Buffer.from(JSON.stringify(value), 'utf8'); }
  catch (error) { throw error instanceof RuntimeFailure ? error : new RuntimeFailure('invalid-input'); }
  if (data.length > INPUT_LIMIT) throw new RuntimeFailure('input-limit');
  parse(data);
  return data;
}
export function readInput(stream: Readable): Promise<Json> {
  return new Promise((resolve, reject) => {
    const chunks: Buffer[] = [];
    let size = 0;
    const timer = setTimeout(() => done(new RuntimeFailure('stdin-timeout')), 3000);
    function done(error?: Error): void {
      clearTimeout(timer);
      stream.removeAllListeners('data');
      stream.removeAllListeners('end');
      stream.removeAllListeners('error');
      stream.pause();
      if (error) { reject(error); return; }
      try { resolve(parse(Buffer.concat(chunks))); } catch (cause) { reject(cause); }
    }
    stream.on('data', (chunk: Buffer) => {
      size += chunk.length;
      if (size > INPUT_LIMIT) done(new RuntimeFailure('input-limit'));
      else chunks.push(Buffer.from(chunk));
    });
    stream.once('end', () => done());
    stream.once('error', () => done(new RuntimeFailure('stdin')));
  });
}
async function execute(handler: Handler, input: Input, deadline: number): Promise<Json> {
  const data = bytes(input);
  const env: NodeJS.ProcessEnv = Object.assign(Object.create(null), process.env);
  for (const [key, value] of Object.entries(handler.env)) {
    if (typeof value === 'string') env[key] = value;
    else {
      const source = process.platform === 'win32' ? Object.keys(process.env).find(name => name.toLowerCase() === value.ref.toLowerCase()) : value.ref;
      if (!source || !Object.hasOwn(process.env, source) || typeof process.env[source] !== 'string') throw new RuntimeFailure('missing-env');
      env[key] = process.env[source];
    }
    if (process.platform === 'win32') {
      for (const ambient of Object.keys(env)) if (ambient !== key && ambient.toLowerCase() === key.toLowerCase()) delete env[ambient];
    }
  }
  const timeout = Math.min(handler.timeout_ms, deadline - performance.now());
  if (timeout <= 0) throw new RuntimeFailure('event-timeout');
  return new Promise((resolve, reject) => {
    let child: ReturnType<typeof start>;
    try { child = start(handler.argv, handler.cwd, env, data); }
    catch { reject(new RuntimeFailure('spawn')); return; }
    let settled = false;
    let stdout = Buffer.alloc(0);
    let stderr = Buffer.alloc(0);
    const timer = setTimeout(() => finish(new RuntimeFailure('timeout')), timeout);
    function cleanup(): void {
      if (child.pid) {
        try {
          if (process.platform === 'win32') child.kill();
          else process.kill(-child.pid, 'SIGKILL');
        } catch { /* The process group may already have exited. */ }
      }
    }
    function finish(error?: RuntimeFailure): void {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      cleanup();
      child.closeInput(); child.stdout.destroy(); child.stderr.destroy();
      if (error) { reject(error); return; }
      try {
        new TextDecoder('utf-8', { fatal: true }).decode(stderr);
        resolve(parse(stdout));
      } catch { reject(new RuntimeFailure('invalid-output')); }
    }
    child.stdout.on('data', (chunk: Buffer) => {
      if (stdout.length + chunk.length > OUTPUT_LIMIT) finish(new RuntimeFailure('stdout-limit'));
      else stdout = Buffer.concat([stdout, chunk]);
    });
    child.stderr.on('data', (chunk: Buffer) => {
      if (stderr.length + chunk.length > ERROR_LIMIT) finish(new RuntimeFailure('stderr-limit'));
      else stderr = Buffer.concat([stderr, chunk]);
    });
    child.exited.then(cleanup, () => finish(new RuntimeFailure('spawn')));
    child.completed.then(code => finish(code === 0 ? undefined : new RuntimeFailure('exit')), () => finish(new RuntimeFailure('spawn')));
  });
}
function textField(value: Json | undefined): value is string {
  return typeof value === 'string' && value.trim().length > 0 && Buffer.byteLength(value) <= 8192;
}
function outcome(value: Json, handler: Handler): RecordValue {
  if (!object(value) || value.version !== 1 || typeof value.outcome !== 'string' || !handler.outcomes.includes(value.outcome)) throw new RuntimeFailure('undeclared-outcome');
  const field = ({ block: 'reason', ask: 'reason', context: 'text', 'modify-input': 'input' } as Record<string, string>)[value.outcome];
  const keys = ['version', 'outcome', ...(field ? [field] : [])];
  if (Object.keys(value).length !== keys.length || Object.keys(value).some(key => !keys.includes(key))) throw new RuntimeFailure('output-fields');
  if (['block', 'ask'].includes(value.outcome) && !textField(value.reason)) throw new RuntimeFailure('reason');
  if (value.outcome === 'context' && !textField(value.text)) throw new RuntimeFailure('context');
  if (value.outcome === 'modify-input' && !(handler.event === 'prompt' ? typeof value.input === 'string' : object(value.input))) throw new RuntimeFailure('replacement');
  return value;
}
export async function run(config: Config, input: Input): Promise<Effects> {
  const deadline = performance.now() + 300000;
  bytes(input);
  const effects: Effects = { contexts: [] };
  const current = structuredClone(input);
  for (const handler of config.handlers) {
    if (handler.event !== input.event || (handler.tools.length && !handler.tools.includes(input.tool ?? ''))) continue;
    const output = outcome(await execute(handler, current, deadline), handler);
    if (performance.now() >= deadline) throw new RuntimeFailure('event-timeout');
    if (output.outcome === 'modify-input') { current.input = output.input; effects.input = output.input; }
    if (output.outcome === 'context') {
      effects.contexts.push(output.text as string);
      if (Buffer.byteLength(effects.contexts.join('\n')) > 8192) throw new RuntimeFailure('context-limit');
    }
    if (output.outcome === 'block' || output.outcome === 'ask') {
      effects.decision = output.outcome; effects.reason = output.reason as string; break;
    }
  }
  if (performance.now() >= deadline) throw new RuntimeFailure('event-timeout');
  return effects;
}
