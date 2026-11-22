# Architecture

[Layers](#layers) · [Native loader](#native-loader) · [Worker pool](#worker-pool) · [Error format](#error-format) · [Panic safety](#panic-safety) · [Value conversion](#value-conversion) · [Cancellation](#cancellation) · [Build output](#build-output)

The TypeScript package is a binding. Every domain decision is made by the Rust
crate; the TypeScript layer validates what only JavaScript can get wrong and
adapts shapes.

## Layers

| Layer                | Location                   | Responsibility                                                                                                                                  |
| -------------------- | -------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------- |
| Rust crate `flyrail` | `../rust`                  | All domain logic, durable formats and error text.                                                                                               |
| Native addon         | `native/` (`flyrail-node`) | napi-rs exports over the crate: argument conversion, the worker pool, tagged results, panic containment and cancel tokens. Built as a `cdylib`. |
| Loader               | `src/native/loader.ts`     | Finds and loads the addon once, at module evaluation.                                                                                           |
| Internal helpers     | `src/internal/`            | Tagged-result unwrapping (`call.ts`), AbortSignal handling (`abort.ts`), path, text and map validation.                                         |
| Public API           | `src/*.ts`, `src/index.ts` | Classes, plain frozen result objects and functions. `index.ts` is the only entry point and never re-exports `internal/` or `native/`.           |

The addon is its own Cargo workspace that depends on `../rust` by path, so the
crate's workspace, lockfile and release are untouched by the binding.

## Native loader

The loader tries, in order:

1. the checkout dev build, `build/native/flyrail.<triple>.node`, resolved from
   `import.meta.url` (`dist/native/loader.js` sits at the same depth as
   `src/native/loader.ts`, so the relative URL works from both);
2. the platform package `flyrail-<os>-<arch>` (`flyrail-darwin-arm64`,
   `flyrail-darwin-x64`, `flyrail-linux-arm64`, `flyrail-linux-x64`), file
   `flyrail.node`;
3. otherwise it throws `NativeLoadError` listing each attempt.

`<triple>` is `darwin-arm64`, `darwin-x64`, `linux-arm64-gnu` or
`linux-x64-gnu`. Any other `process.platform`/`process.arch` throws
`NativeLoadError` before any attempt. Both candidates load through
`createRequire(import.meta.url)(path)`, never a raw `process.dlopen`, so
Electron's patched module loader redirects an `app.asar` path to
`app.asar.unpacked`. The design spike confirmed this with Electron 45.2.3
running as Node: an addon packaged in an asar archive loaded from the unpacked
directory. There is no environment-variable override for the library path,
because the library never reads the environment.

## Worker pool

Every call that touches the filesystem or a lock returns a Promise. Its work
runs on a pool owned by the addon, not on libuv's thread pool, which has four
threads by default and is shared with Node's own filesystem and DNS work; a
handful of lock waits would otherwise starve the host application. The pool
starts empty and spawns a worker (named `flyrail-worker-<n>`, 16 MiB stack)
whenever the queued jobs outnumber idle workers, up to 64. Jobs run in FIFO
order. Each job carries its cancel token: a worker that dequeues a job whose
token is already cancelled settles it as aborted without running it. If a
worker cannot be spawned and no idle worker can take the job, the job settles
at once with a `native` error rather than waiting forever.

Results resolve a napi `JsDeferred`, whose resolver runs on the JavaScript
thread and converts the Rust value there. Closures handed to the pool own
`Arc` clones of everything they use and never borrow from a JavaScript
wrapper.

In the design spike, eight concurrent lock waits on Node 24.21.0, Bun 1.4.2
and Electron's Node runtime left the event loop responsive (71 to 75 ticks of
a 10 ms interval timer during the wait), and aborted waits settled 14 to 23 ms
after the abort. Terminating a `worker_threads` Worker with a job in flight
neither crashed nor hung the process on any of those runtimes.

Synchronous calls whose input or output can nest (value conversion, rendering,
translation, document editing, hook JSON parsing) run on a short-lived
16 MiB-stack thread that the JavaScript thread joins, so deep input cannot
overflow the JavaScript thread's stack. The TypeScript value converter refuses
nesting deeper than 512 levels before any native call, and the native
conversion checks the same bound. The spike measured that a 512-deep
conversion on the JavaScript thread overflows Node's main stack in an
unoptimized build, so the debug profile uses `opt-level = 1`.

## Error format

Every fallible native entry point, synchronous or asynchronous, returns a tagged
object built with napi object APIs:

```ts
type NativeResult<T> = { ok: true; value: T } | { ok: false; error: NativeErrorPayload };
```

Asynchronous entry points resolve their Promise with it and never reject with a
domain error. `src/internal/call.ts` unwraps the result and throws the mapped
class: `value` and `argument` payloads become `ValueError`, `bundle`
`BundleError`, `authority` `AuthorityError`, `store` `StoreError`, `hook`
`HookError`, `unsupported-translation` `UnsupportedTranslationError`, `panic`
`NativePanicError` and `native` `NativeError`. Absent optional payload fields
become `null`. Anything thrown or rejected that is not a tagged result, such as
an argument-conversion error raised by napi-rs itself, becomes `NativeError`
carrying the original message. No error is ever encoded into a message string
and parsed back, so the error kind never depends on message text.

## Panic safety

A Rust panic must never unwind into the JavaScript engine. Three layers ensure
it:

- every exported function, method and constructor is declared
  `#[napi(catch_unwind)]` (`test/napi-attributes.test.ts` fails on any that is
  not);
- every fallible entry point runs its work inside `guarded`, which catches the unwind
  and returns a `panic` payload carrying the panic message, and the deferred
  resolver's conversion to JavaScript runs inside `catch_unwind` too;
- a silent panic hook is installed once when the addon is loaded, so panics do
  not print to stderr of the host process while their message still reaches
  JavaScript as `NativePanicError`.

A worker that catches a panic stays in the pool and takes the next job.

## Value conversion

Semantic values cross as `{ t, ... }` trees. Both directions (`toNative` and
`fromNative` in `src/internal/convert.ts`, `ValueTree` in `native/src/convert.rs`)
walk the tree with an explicit stack instead of recursion, because a 512-deep
recursive conversion overflowed Node's main-thread stack in the design spike.
The 512-level bound is checked in TypeScript and again natively; the native
check returns an `argument` payload, which becomes `ValueError`, because the
crate does not let the addon construct its own `ValueError`. Work that recurses
inside the crate (encoding, equality, manifest values) runs on the 16 MiB
`flyrail-stack` thread.

## Cancellation

The crate provides `flyrail::cancellation::{Cancellation, Cancelled, cancellable}`:
a thread-scoped token honoured by every lock wait and checked once more right
after a transaction's locks are acquired. The addon exposes it as
`CancelToken`. `runMutation` (`src/internal/abort.ts`) creates a token for a
call that has an `AbortSignal` and cancels it when the signal aborts; the
native side checks the token before dispatching work, the worker checks it when
it dequeues the job, and the operation runs inside `cancellable`.

For a mutation, Rust's `Err(Cancelled)` becomes a rejection with
`signal.reason`. `Err(Cancelled)` means no index, receipt, journal or
preparation was written, no destination content was published or removed, and
no recovery ran. Lock files, store/staging/destination parent directories and
authority records (which the transaction protocol retains permanently anyway)
may persist, the same footprint a busy timeout leaves. Once the first durable
write begins the token is ignored, so a mutation whose signal fires after that
point resolves with its real result. The 10 s mutation lock inside a single
`DirectoryStore` write is not cancellable, so cancellation latency can reach
that bound.

Read-only calls reject with `signal.reason` as soon as the signal aborts; the
pool job finishes in the background and its result is discarded. Both helpers
remove their abort listener when the call settles, and an already-aborted
signal rejects before any native call.

## Build output

tsdown builds `dist/` in unbundle mode, so `dist/` mirrors `src/` and
`dist/native/loader.js` keeps its relative path to `build/native/`. The
generated `src/native/binding.d.ts` types the addon for the internal modules
only; no public declaration imports it.
