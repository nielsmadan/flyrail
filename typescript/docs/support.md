# Support and packaging

[Platforms and runtimes](#platforms-and-runtimes) · [Installation](#installation) · [Native addon loading](#native-addon-loading) · [Electron](#electron) · [Process lifecycle](#process-lifecycle) · [Bun](#bun) · [Paths and text](#paths-and-text) · [Shared fixtures](#shared-fixtures) · [Deferred](#deferred)

## Platforms and runtimes

| Platform              | Status                                        |
| --------------------- | --------------------------------------------- |
| macOS arm64 and x64   | Supported                                     |
| Linux glibc arm64/x64 | Supported                                     |
| Linux musl            | Unsupported                                   |
| Windows               | Unsupported (`package.json` `os` excludes it) |

| Runtime  | Version                             | Tested by                                                    |
| -------- | ----------------------------------- | ------------------------------------------------------------ |
| Node.js  | `>=24.14` (`engines.node`)          | CI pins 24.21.0; `just check`, `just check-package`          |
| Bun      | 1.4.2 or newer                      | `just bun-smoke` and the package QA's Bun consumer, on 1.4.2 |
| Electron | 45.2.3 (main process, Node 24.22.2) | The package QA's asar-packed Electron app, on macOS only     |

Node 24.14 fixed threadsafe-function teardown, which every asynchronous result
relies on, so earlier Node 24 releases are unsupported. Electron is tested on
macOS only; Linux Electron is untested.

Linux addons are built natively on the machine or runner that builds them. An
addon built on CI's ubuntu-24.04 runner needs that runner's glibc, 2.39, or
newer; one built by `just check-linux` in `node:24.21.0-bookworm` needs glibc
2.36 or newer. A release prebuild pipeline will set a lower baseline; until it
exists, build the addon on a system no newer than the oldest one you deploy to.

## Installation

Nothing is published to npm. Until release automation exists, install two
local tarballs together: the `flyrail` package and the platform package
`flyrail-<os>-<arch>` that carries `flyrail.node`. `just pack` in `typescript/`
builds both (see the [README](../README.md#installation)):

```sh
npm install /path/to/flyrail-0.1.0.tgz /path/to/flyrail-darwin-arm64-0.1.0.tgz
```

The platform package declares `os`, `cpu` and, on Linux, `libc: ["glibc"]`.
`flyrail` has no `optionalDependencies` on platform packages yet, so installing
from a tarball never tries to fetch unpublished packages.

## Native addon loading

`flyrail` loads its addon once, when the package is first imported. The loader
tries the checkout's debug build (`build/native/flyrail.<triple>.node`, absent
from the package) and then `flyrail-<os>-<arch>/flyrail.node`, resolved and
loaded through `createRequire(import.meta.url)`; if neither loads it throws
`NativeLoadError` listing both attempts. Loading through `createRequire`
rather than `process.dlopen` lets Electron redirect an `app.asar` path to
`app.asar.unpacked`.

There is no environment variable that overrides the addon path. The library
never reads the environment, so the addon's location is decided only by
package resolution.

## Electron

Electron main processes are supported when three rules hold:

- `flyrail` stays external to the main-process bundle, and both `flyrail` and
  `flyrail-<os>-<arch>` are copied into the packaged app's `node_modules`. The
  loader resolves the platform package at run time, so a bundler never sees it.
- Every `.node` file is unpacked from the asar archive with the pattern
  `**/*.node`. A pattern such as `*.node` misses nested files.
- The main process is an ES module (`"type": "module"` or a `.mjs` entry):
  `flyrail` is ESM only. Start work with `app.whenReady().then(...)` rather
  than a top-level `await app.whenReady()`.

| Tool                    | Configuration                                                                                                                                 |
| ----------------------- | --------------------------------------------------------------------------------------------------------------------------------------------- |
| esbuild                 | `--platform=node --format=esm --external:electron --external:flyrail --external:flyrail-*`                                                    |
| Vite                    | `build.rollupOptions.external: ['electron', 'flyrail', /^flyrail-/]` and an ESM output format for the main entry                              |
| electron-vite           | Keep `flyrail` in `dependencies` so dependency externalization leaves it out of the main bundle, or list it in `build.rollupOptions.external` |
| electron-builder        | Keep both packages in `dependencies` (it copies production dependencies) and set `asarUnpack: ["**/*.node"]` explicitly                       |
| Electron Forge + Vite   | The Vite plugin does not copy externals into the package ([electron/forge#3738](https://github.com/electron/forge/issues/3738)); see below    |
| @electron/asar directly | `asar pack app app.asar --unpack "**/*.node"`, run from the app's parent directory with a relative source path                                |

With Electron Forge's Vite plugin, copy `node_modules/flyrail` and
`node_modules/flyrail-<os>-<arch>` into the packaged app yourself, for example
in a `packageAfterCopy` hook that copies both directories into
`<buildPath>/node_modules/`, and set `packagerConfig.asar` to
`{ unpack: '**/*.node' }`.

@electron/asar 4.3.1 unpacked nothing when the source directory's absolute path
contained a dot-directory (such as `.cache`); packing from the parent directory
with a relative source path avoids it. Check the result with `asar list
--is-pack app.asar`, which must mark every `.node` entry `unpack`, and check
that `app.asar.unpacked/node_modules/flyrail-<os>-<arch>/flyrail.node` exists.
`just check-package` builds such an app and asserts both, and that the addon
was loaded from `app.asar.unpacked`.

## Process lifecycle

A pending call keeps the event loop alive until it settles, so await every
mutation before calling `process.exit` or Electron's `app.exit`. Exiting
earlier abandons the operation wherever it is; the next `sync`, `remove` or
`recoverInstallation` recovers it. Calling the package from `worker_threads` is
supported: the design spike terminated a worker with a call in flight on Node,
Bun and Electron without a crash or a hang. See
[lifecycle](lifecycle.md#process-lifecycle).

## Bun

Bun 1.4.2 is the tested minimum. Run code that uses `flyrail` with `bun run`,
never `bun test`: `bun test --isolate` and `--parallel` have environment
lifecycle bugs. The package resolves and loads
under Bun exactly as under Node.

## Paths and text

Paths cross into Rust as UTF-8 strings. Paths coming back from Rust (target
roots, recovery paths, store locations, discovery results) are converted
lossily: a path that is not valid UTF-8 has its invalid bytes replaced with
U+FFFD, so it cannot be passed back to `flyrail` and reach the same file.
Strings that become Rust strings must be well formed (`isWellFormed()`), or the
call throws `ValueError`. Hook text, which may hold lone surrogates, crosses as
UTF-16 instead.

## Shared fixtures

The conformance suite consumes every `spec/fixtures/*.json` file except
`interop-source-options.json`, which constrains the `write --source` option of
interop drivers. `differential.tsv` is likewise driver-only. The TypeScript
package ships no interop driver, so neither applies; cross-language behaviour
is checked against the Rust and Python drivers instead
([development](development.md#interop)).

## Deferred

- Stores implemented in JavaScript (host stores) and the store conformance
  helper `testing::check_store`. Only `DirectoryStore` and `MemoryStore` are
  offered.
- `StateStore::lock`: a borrowed Rust lock guard cannot cross the asynchronous
  boundary, so store locks are not exposed.
- Release automation: npm publishing, multi-arch prebuilds, a glibc baseline,
  and `optionalDependencies` on the platform packages (npm's optional
  dependency handling, npm/cli#4828, needs checking then).
- Linux Electron, Windows and musl.
