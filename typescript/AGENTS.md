# Flyrail for TypeScript

The `flyrail` npm package is an ESM-only, async TypeScript API for Electron main
processes, Node CLIs and Pi/OpenCode plugins. It is a binding, not a port: the
engine is the Rust crate in `../rust`, compiled into the napi-rs addon
`native/` (`flyrail-node`). The TypeScript layer reimplements no domain logic.
It validates JavaScript-specific input (absolute paths, well-formed strings,
i64 ranges, finite timeouts), adapts shapes, and maps the native tagged results
to error classes. Behaviour, durable formats and error text are Rust's.

Library code never prints, prompts, exits, spawns processes, reads
`process.env`, `process.cwd()` or `os.homedir()`, or touches the network. Home,
environment, PATH and working directory arrive only as explicit arguments.
ESLint enforces this on `src/**`; do not weaken those rules. Every `#[napi]`
function, method and constructor carries `catch_unwind`
(`test/napi-attributes.test.ts` fails otherwise). Every fallible entry point
runs its work inside `guarded` and returns a tagged `{ ok, value | error }`
object; only the infallible exports (`nativeVersion`, `targetsAgents`,
`hookLimits`) and `CancelToken` are untagged. Never encode an error into a message string. Blocking work runs on the crate's own worker pool,
never on libuv's. `src/index.ts` is the public entry and `src/errors.ts` is also exported as
`flyrail/errors`, which must never load the addon; `src/internal/**` and
`src/native/**` are never re-exported. The public API carries no doc comments:
`docs/api.md` is the reference. Read `docs/architecture.md` before changing the
pool, the loader or the error format.

Run everything through `just` from this directory; the Justfile points
`CARGO_HOME`, `CARGO_TARGET_DIR` and the npm cache at the repository-root
`.cache/`. `just setup` installs the locked npm dependencies. `just check` runs,
in order, the dependency check, the debug native build (which generates the
ignored `src/native/binding.d.ts`), the Prettier and rustfmt checks, ESLint and
clippy, the type check and the unit, smoke and conformance tests; it is part of
the git-hook gate and must stay fast. `just check-integration` adds the
integration tests and the Bun smoke script (`just bun-smoke`). `just check-package`
runs the package QA (`scripts/package_qa.py`: packed tarballs installed into Node,
Bun and, on macOS, an asar-packed Electron app), and `just check-linux` runs the
fast tier and the Node and Bun package QA in a Linux container. `just build` builds `dist/` with tsdown, and `just lock`
re-seeds `native/Cargo.lock` from `../rust/Cargo.lock`. CI runs the fast tier on every selected change and the integration and package tiers
nightly (see `docs/development.md`). There is no release automation: nothing is published.

`native/` is its own Cargo workspace with its own `Cargo.lock`, seeded from
`../rust/Cargo.lock`. A package that appears in both lockfiles must resolve to
the same versions; `just deps-check` fails on drift. Keep the clippy policy in
`native/Cargo.toml` identical to `../rust/Cargo.toml`.

Keep caches, temporary files and test installations inside this checkout.
Tests create temporary roots under `../.cache/typescript/tmp/` with
`test/helpers/temp.ts`, never in the real home directory or agent
configuration. Do not change HOME or install global tools. Make positive
assertions, and read the full vitest summary and exit status before reporting.

Shared conformance fixtures live in `../spec/fixtures`; when a fixture is
absent, tests skip with a notice unless `FLYRAIL_REQUIRE_FIXTURES=1`, which the
Justfile sets. Tool-dependent tests resolve Node, Bun and the repository Python
through `test/helpers/tools.ts`. Hook runtime checks require Node and Bun; CI
pins Node 24.21.0, npm 11.19.0 and Bun 1.4.2, and Bun scripts run with
`bun run`, never `bun test`.
