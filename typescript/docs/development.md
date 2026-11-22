# TypeScript development

[Toolchain](#toolchain) · [Commands](#commands) · [Repository-local caches](#repository-local-caches) · [Native crate and lockfile](#native-crate-and-lockfile) · [Type checking and builds](#type-checking-and-builds) · [Tests](#tests) · [Interop](#interop) · [Package QA](#package-qa) · [Linux](#linux) · [CI](#ci-and-maintenance)

Run commands from `typescript/`. The repository-root Justfile runs `just setup`
and `just check` here as part of its own `setup` and `check`, and
`check-integration` and `check-package` here as part of its own. Warm
`just check` here takes 19 s (median of three runs: 19, 19 and 18 s on Apple
silicon with everything cached), inside the 30 s budget for the git-hook gate.
If a later change pushes it past 30 s, native clippy and the `conformance`
project move into `just check-integration`; format-check, lint, type check and
the `unit` and `smoke` projects stay in `just check`.

## Toolchain

| Tool       | Version                                                                       |
| ---------- | ----------------------------------------------------------------------------- |
| Node.js    | `>=24.14` (`engines.node`); CI pins 24.21.0                                   |
| npm        | 11.19.0 in CI, with the committed `package-lock.json`                         |
| Bun        | 1.4.2, the minimum the design spike verified (run scripts with `bun run`)     |
| Rust       | 1.99.0 from the root `rust-toolchain.toml`                                    |
| TypeScript | 6.0.3, the newest release inside typescript-eslint 8.72.1's peer range (<6.1) |

Node 24.14 fixed threadsafe-function teardown, which every pool result relies
on, so earlier Node 24 releases are unsupported. `package.json` pins every
development dependency exactly. The repository's other TypeScript tooling pins
TypeScript 7, but the lint stack does not support it yet, so this package
type-checks with 6.0.3.

## Commands

| Command                     | Work performed                                                                                                                                                                                         |
| --------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `just setup`                | `npm ci`.                                                                                                                                                                                              |
| `just check`                | In this order: `deps-check`, `native-build`, `format-check`, `lint`, `typecheck`, `test`. The native build comes first because it generates `src/native/binding.d.ts`, which lint and type check read. |
| `just check-integration`    | `just check`, `just interop-driver`, `just tools`, then the `integration` vitest project and `just bun-smoke`.                                                                                         |
| `just interop-driver`       | Fails unless `../python/.venv` exists, then builds the Rust `interop` example into `../rust/target` (without this package's `CARGO_TARGET_DIR`).                                                       |
| `just bun-smoke`            | Runs `test/runtime/bun-smoke.ts` with `bun run` against the debug addon (see [Tests](#tests)).                                                                                                         |
| `just deps-check`           | Fails with "run just setup" unless `node_modules` matches the lockfile (`npm ls --depth=0`), then runs the Cargo lockfile drift check.                                                                 |
| `just native-build`         | Debug addon with the `test-seams` feature into `build/native/flyrail.<triple>.node`; generates `src/native/binding.d.ts`.                                                                              |
| `just native-build-release` | Optimized, stripped addon without test seams into `../.cache/typescript/release/`, with its declarations beside it.                                                                                    |
| `just format`               | Prettier and rustfmt rewrite files.                                                                                                                                                                    |
| `just format-check`         | Prettier and rustfmt check without rewriting.                                                                                                                                                          |
| `just lint`                 | ESLint, then clippy on the addon with all features and `-D warnings`.                                                                                                                                  |
| `just typecheck`            | `tsc --noEmit`.                                                                                                                                                                                        |
| `just test`                 | The `unit`, `smoke` and `conformance` vitest projects.                                                                                                                                                 |
| `just build`                | tsdown into `dist/`.                                                                                                                                                                                   |
| `just lock`                 | Refreshes `package-lock.json` and re-seeds `native/Cargo.lock` (below).                                                                                                                                |
| `just tools`                | Installs the pinned Bun 1.4.2 into `../.cache/tools/` (see [Tests](#tests)).                                                                                                                           |
| `just check-package`        | `native-build-release`, `build`, then the [package QA](#package-qa).                                                                                                                                   |
| `just pack [DIR]`           | `native-build-release`, `build`, then `npm pack`s the package and its platform package into `DIR`.                                                                                                     |
| `just check-linux`          | The fast tier and the Node and Bun package QA in a Linux container (see [Linux](#linux)).                                                                                                              |

## Repository-local caches

The Justfile exports `CARGO_HOME=../.cache/cargo-home`,
`CARGO_TARGET_DIR=../.cache/typescript/target` and
`npm_config_cache=../.cache/npm` (as absolute paths), plus
`FLYRAIL_REQUIRE_FIXTURES=1`. Test temporary roots live under
`../.cache/typescript/tmp/`. Generated outputs (`build/`, `dist/`,
`src/native/binding.d.ts`, `node_modules/`) are ignored.

## Native crate and lockfile

`native/` (`flyrail-node`) is a separate Cargo workspace with `publish = false`
and the clippy policy copied verbatim from `../rust/Cargo.toml`. Its library
target is a `cdylib` with `test = false` and `bench = false`: napi-derive omits
export registration under `cfg(test)`, so a test build of the addon would only
report dead code. The debug profile uses `opt-level = 1`, because an
unoptimized build overflows Node's main stack on deep value conversions.

`native/Cargo.lock` is seeded by copying `../rust/Cargo.lock` and letting cargo
add the napi dependencies, so the engine is built with exactly the versions the
Rust crate is tested with. `scripts/check_cargo_lock.py` (run by `deps-check`)
fails, naming each package, when a package present in both lockfiles resolves
to a different set of versions. The napi crates are locked to napi 3.16.1,
napi-derive 3.7.0 and napi-derive-backend 7.0.0: napi-derive-backend 7.0.1
generates code that needs napi 3.16.2. `just lock` re-seeds from the Rust
lockfile and re-applies those three pins.

## Type checking and builds

The napi CLI resolves `--dts` relative to `--output-dir`, so the debug build
passes `--dts ../../src/native/binding.d.ts` and the release build writes its
declarations next to the release addon. The `napi.dtsHeader` setting in
`package.json` imports the `NativeResult` and `Agent` types into the generated
declarations. tsdown 0.23.7 needs `fixedExtension: false` to emit `.js` and
`.d.ts` instead of `.mjs` and `.d.mts`. TypeScript resolves the bare `flyrail`
specifier in tests to `src/index.ts` through `paths`, and vitest through an
alias.

## Tests

vitest runs four projects, all in the `forks` pool:

| Project       | Files                                                                   | Timeout |
| ------------- | ----------------------------------------------------------------------- | ------- |
| `unit`        | `test/**/*.test.ts` except integration, conformance and the smoke suite | 10 s    |
| `smoke`       | `test/smoke.test.ts`: one minimal real call per bound domain            | 10 s    |
| `conformance` | `test/conformance/**/*.test.ts`                                         | 10 s    |
| `integration` | `test/**/*.integration.test.ts`                                         | 60 s    |

The addon's test seams (`internalPanic`, `internalPanicAsync`, `internalBlock`,
`internalCancelProbe`, `internalRendezvous`, `internalHoldLock`, `internalCanonicalState`) exist only in
the debug build. `internalHoldLock(store, name)` acquires a store lock on its
own thread, outside the worker pool, and resolves a handle once the lock is
held; tests call its `release()` in `finally`. Integration tests pass an
explicit per-test timeout and a `lockTimeoutMs` (10 s) wherever they expect a
blocked wait. `test/helpers` provides
`tempRoot()` (a `0700` directory removed when the test finishes),
`fixture(name)` for `../spec/fixtures`, and `requireTool`/`toolAvailable` for
Node, Bun and the repository Python.

`just bun-smoke` runs `test/runtime/bun-smoke.ts` with `bun run` (never
`bun test`): eight concurrent `sync` calls sharing one `DirectoryStore`, and one
`sync` aborted while `internalHoldLock` holds its index lock. It exits non-zero
on any mismatch. `just tools` (run by `check-integration` and `bun-smoke`)
downloads Bun 1.4.2 with `scripts/tools.py`, verifying the archive's SHA-256,
into `../.cache/tools/bun-1.4.2/bun`. When that fails, the recipes and
`test/helpers/tools.ts` fall back to the `bun` on `PATH`, which CI pins to
1.4.2. The smoke script prints the Bun version it ran.

## Interop

`test/interop.integration.test.ts` drives the Rust example
`../rust/target/debug/examples/interop` (built by `just interop-driver`) and
`../python/scripts/interop_driver.py` (run with `../python/.venv/bin/python`),
each invocation under a 60 s timeout. It checks that a TypeScript installation
is current and removable through Rust, that a Rust installation is current and
removable through TypeScript, that an interrupted Rust update is pending in
TypeScript and survives `recoverInstallation`, that TypeScript and Python
sections coexist in one document, and that `internalCanonicalState` agrees with
`interop encode --type Receipt` on every fixture receipt. When a driver is
absent the tests skip with a notice unless `FLYRAIL_REQUIRE_FIXTURES=1`, which
the Justfile sets.

`test/conformance/state-records.test.ts` relocates the `configurations.json`
receipts, journals and preparations into a temporary root (recomputing claim
identities) and asserts that `internalCanonicalState` is a fixed point on its
own output. Guarded recovery semantics are exercised by the Rust engine suite.

## Package QA

`just check-package` builds the release addon and `dist/`, then runs
`scripts/package_qa.py` (standard library only, `python3 -I -B`). All of its
work happens in a fresh `0700` directory under
`../.cache/typescript/package-qa/`, with the npm and Electron caches under
`../.cache/`; the directory is removed when every step passes and kept, with
each consumer's stdout and stderr, when one fails.

1. It checks that the release addon and `dist/` exist, `npm pack`s the package
   and asserts that the tarball holds exactly `dist/**`, `package.json`,
   `README.md` and `LICENSE`.
2. It generates the platform package `flyrail-<os>-<arch>` (`os`, `cpu`,
   `libc: ["glibc"]` on Linux, `files: ["flyrail.node"]`) around the release
   addon and packs it.
3. It runs `publint --strict --pack npm` and `attw --profile esm-only` on the
   tarball.
4. It installs both tarballs into a consumer directory and runs
   `scripts/consumer/node-consumer.mjs` with Node, then with the pinned Bun
   (`bun run`). The consumer loads the example bundle from a directory,
   installs it into a directory target, inspects it, runs eight concurrent
   `sync`s, aborts a blocked `sync`, and uninstalls, printing one JSON report
   whose every field the QA asserts. For Node the QA also asserts that the
   addon was loaded from the platform package; Bun has no `process.report`, so
   that check is skipped for Bun.
5. On macOS it builds an Electron 45.2.3 app from `scripts/consumer/electron`:
   esbuild bundles `main.ts` with `flyrail` external, the two packages are
   copied into the app, and @electron/asar packs it with `--unpack "**/*.node"`.
   It asserts that `asar list --is-pack` marks every `.node` entry `unpack`
   and that the platform addon is in `app.asar.unpacked`. It then runs the consumer from inside
   `app.asar` with `ELECTRON_RUN_AS_NODE=1`, and finally launches the app as an
   Electron main process (no window, 120 s limit). Both runs must report that
   the addon was loaded from `app.asar.unpacked`.

The blocked `sync` is deterministic: the release addon has no test seams, so
the QA holds the lock itself. The consumer syncs the abort target once, then
writes the index lock name (`index/<bundle-id>/<contextDigest>`) to a handshake
file. The QA opens `<store>/locks/<name with '/' replaced by '~'>`, the
`DirectoryStore` lock file, takes `fcntl.flock(LOCK_EX)` (Rust's
`File::try_lock` is `flock` on macOS and Linux) and signals the consumer, which
starts a `sync` with `lockTimeoutMs: 10000` and aborts it after 100 ms. The
rejection must be the abort reason itself and arrive within 2 s. After the
consumer reports it, the QA releases the lock, and the consumer's next `sync`
of the target must be `unchanged`.

The QA exits 0 when every step passes, 1 when any fails, and 3 when nothing
failed but a step was BLOCKED. An Electron main process cannot start inside
the nono sandbox (it aborts with `bootstrap_check_in
com.github.Electron.MachPortRendezvousServer: Permission denied (1100)`); the
QA reports that step as BLOCKED, still runs every other step, and names the
command to run outside the sandbox: `cd typescript && just check-package`.

## Linux

`just check-linux` verifies Linux locally through Docker (Colima on macOS). It
runs `scripts/check_linux.sh` in `node:24.21.0-bookworm` as the calling user,
with the checkout mounted read-only and every cache under
`../.cache/typescript/linux/` (`HOME`, `TMPDIR`, `RUSTUP_HOME`, `CARGO_HOME`,
`CARGO_TARGET_DIR` and the npm cache). The script copies `typescript/` (without
`node_modules`, `build` and `dist`), `rust/`, `spec/` and
`rust-toolchain.toml` into the cache, so host dependencies are never reused,
installs the pinned Rust toolchain with rustup on first use, and runs `npm ci`,
the fast tier in `just check` order, the release addon build, `dist/` and the
package QA, whose Electron steps are skipped on Linux. When the Docker daemon
is not running, start Colima (`colima start`) and rerun.

## CI and maintenance

[CI](../../.github/workflows/ci.yml) runs the TypeScript fast job on Ubuntu 24.04
and macOS 26 for changes under `typescript/` and every path that selects Rust; see
the [path selection table](../../python/docs/development.md#ci-and-maintenance).
The binding builds the Rust crate, so changes to `rust/` run it too.

| Job                           | When                                                  | What                                                                                                                                                                                                                                                                     |
| ----------------------------- | ----------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `TypeScript (os)`             | Pushes, nightly, dispatch, and affected pull requests | Installs the Rust toolchain from `rust-toolchain.toml`, Node 24.21.0 and npm 11.19.0 (asserted), then `npm ci` and the `just check` order: dependency check, native build, Prettier and rustfmt, ESLint and clippy, type check and tests, followed by `npm run build`.   |
| `TypeScript integration (os)` | Nightly schedule and dispatch                         | Provisions Python, uv, Rust, Node 24.21.0 and Bun 1.4.2, builds the Rust interop example, then runs the fast tier, the integration tests, the Bun smoke script, the release addon, `dist/` and the package QA, whose Electron main process runs unsandboxed on macOS 26. |

Both jobs count toward `CI result`, which is the single required status check.
CI runs the underlying commands directly, not through `just`, with the same
`CARGO_HOME`, `CARGO_TARGET_DIR`, `npm_config_cache` and
`FLYRAIL_REQUIRE_FIXTURES` as the Justfile, under the workspace `.cache/`. The
package QA exits 3 when a step is blocked, which fails the job. There is no
release workflow: nothing is published. Node, npm and Bun are pinned in CI, and
[Dependabot](../../.github/dependabot.yml) groups weekly `npm` updates under
`/typescript` and one `cargo` update for `/rust` and `/typescript/native` together, so shared crates
move in both lockfiles in the same pull request. Updating npm
packages or napi crates goes through `just lock`, which re-applies the napi pins.
