# Go development

[Tool setup](#tool-setup) · [Commands](#commands) · [Repository-local caches](#repository-local-caches) · [Linux runs](#linux-runs) · [Interop prerequisites](#interop-and-runtime-prerequisites) · [Package QA](#package-qa) · [CI](#ci-and-maintenance) · [Releasing Go](#releasing-go)

Run commands from `go/`. The repository-root Justfile delegates to the same
checks. Use the Go toolchain the Justfile selects, `go1.27.3`; the module is
`github.com/nielsmadan/flyrail/go` with `go 1.27` in `go.mod`. Run Go only
through `just`: the recipes set every environment variable the build needs, and
a bare `go` call would write its caches outside the checkout. The few `go`
commands shown in these docs assume the variables from
[Repository-local caches](#repository-local-caches) are exported.

## Tool setup

`just tools` downloads golangci-lint 2.15.0 into `../.cache/tools/` with a
checksum check (`scripts/tools.py`); recipes that need it run `tools` first.
`just audit-dependencies` runs `govulncheck` v1.9.0 through `go run`. The
Python recipes (`case-folding`, the interop scripts) use `uv` and the
repository's locked Python environment, provisioned from the repository root with:

```sh
python3 python/scripts/check.py sync
```

## Commands

| Command | Work performed |
| --- | --- |
| `just check` | Generated-table and errno-text staleness, format check, `go vet`, golangci-lint for `GOOS=darwin` and `GOOS=linux`, builds and vets for darwin and linux on amd64 and arm64 with cgo off plus windows and freebsd, the short test tier, and the case-folding cross-check against Python. The git-hook gate. |
| `just check-integration` | The fast gate, the state-vector check, the Python/Rust differential, transaction interop and native interop scripts, then `go test -race -count=1 -tags integration ./...`. |
| `just check-package` | `go mod verify`, then package QA (below). |
| `just check-linux` | The fast tier in a `golang:1.27.3` container. |
| `just check-linux-integration` | The race-enabled integration tier in the same container, with cgo on. |
| `just generated-check` | The generated case-folding table and errno text are current. |
| `just format-check` | Check formatting without rewriting files. |
| `just vet` | `go vet` for the host platform. |
| `just cross-build` | Build and vet for darwin and linux on amd64 and arm64 with cgo off, plus windows and freebsd. |
| `just case-folding` | The case-folding cross-check against Python. |
| `just test-short` | `go test -short ./...`. |
| `just tools` | Download the checksum-verified golangci-lint. |
| `just differential` | The differential corpus through Python, Rust and Go; byte-identical lines are required for the categories in `scripts/differential_categories.txt`. |
| `just transaction-interop` | Python, Rust, Swift (on macOS) and Go run the shared transaction scenarios, each with its own store, and results are compared. |
| `just native-interop` | The same for native security transitions and the hook runtime. |
| `just state-vectors-check` | The state-record vectors extracted from Python match `flyrail/testdata/state_vectors.tsv`. |
| `just generate` | Regenerate the case-folding table and errno text (`go generate`). |
| `just format` | Format Go code with golangci-lint's formatters (gofmt, goimports). |
| `just lint` | Run golangci-lint for darwin and linux. |
| `just test` | The whole suite, race-enabled, with the `integration` tag. |
| `just audit-dependencies` | Run govulncheck. |
| `just lock` | `go mod tidy`. |

`just check-integration` provides the same coverage CI runs nightly. The root
`just check`, `just check-integration`, `just check-package` and
`just audit-dependencies` include Go. Git hooks run the fast gate; the
integration tier belongs in CI and before accepting a release.

The fast tier runs fuzz targets on their seed corpora only. The shared
conformance fixtures live in `../spec/fixtures`; the Justfile sets
`FLYRAIL_REQUIRE_FIXTURES=1`, so a missing fixture fails instead of skipping.
Do not export `CGO_ENABLED=0` globally: the race detector needs cgo.

## Tests

Tests are table-driven `testing` with `go-cmp` as the only test dependency, and
use positive assertions. The fast tier is `-short`; the `integration` build tag
selects only the slow tier. Fault and barrier injection (crash points named as in
Rust) travels in the `context.Context` through `internal/seams`; there are no
other build tags and no package-level mutable hooks, so tests may use
`t.Parallel`. Simulated crashes never rely on `defer` for rollback: cleanup runs
on explicit error paths, and `defer` is only for unlock and close. Cross-process
tests re-exec the test binary from `TestMain` with a ready handshake over an
inherited pipe, and assert only lower bounds on lock timing. Fuzz targets cover
the JSON reader, hook parser, JSONC editor, TOML scanner and editor, ZIP reader
and state codec. The `differential` driver reports errors on stderr with exit 1
like the other languages' differential drivers; every other driver subcommand
prints `{"error":...}` on stdout.

## Repository-local caches

The Justfile exports these variables, all pointing into the repository-root
`.cache/`, so nothing is written to the home directory:

| Variable | Value |
| --- | --- |
| `GOPATH` | `.cache/go` |
| `GOMODCACHE` | `.cache/go/mod` |
| `GOCACHE` | `.cache/go/build` |
| `GOFLAGS` | `-modcacherw` |
| `GOTOOLCHAIN` | `go1.27.3+auto` |
| `GOLANGCI_LINT_CACHE` | `.cache/go/lint` |
| `FLYRAIL_REQUIRE_FIXTURES` | `1` |

`GOPATH` is redirected because the toolchain download otherwise writes its
checksum cache below `~/go`. Downloaded tools live in `.cache/tools`; the
interop driver builds to `.cache/go/bin/flyrail-interop`. Tests use temporary
roots resolved through symlinks and never touch the real agent configuration.

## Linux runs

`just check-linux` and `just check-linux-integration` need a running Docker
daemon (any engine) and the `golang:1.27.3` image. They start the image with the
repository mounted read-only, a non-root user, `GOTOOLCHAIN=local`, and
container-local `HOME`, `GOCACHE`, `GOMODCACHE` and `TMPDIR` under `/tmp`. The
fast run also checks the generated errno text against the container's live
`os.strerror`, which is how glibc's wording differs from libSystem's.

If your environment cannot pull with a credential helper, pull the public image
once with `docker pull golang:1.27.3`.

## Interop and runtime prerequisites

`just check-integration` uses `../python/.venv/bin/python` and builds the Rust
examples with `cargo` (a missing `cargo` skips the Rust side locally, but under
`CI=true` it, and `swift` on macOS, fails the run). On macOS the scripts also
build the Swift driver. Hook runtime tests need Node 24+, npm and Bun; CI uses
exactly Node **24.21.0**, npm **11.19.0** and Bun **1.4.2**.

The Go side of every cross-language check is `internal/cmd/flyrail-interop`, a
driver. Its subcommands `encode`, `write`, `install`, `update`, `inspect`,
`remove`, `recover`, `records` and `hold` are the contract shared with the
Python and Rust drivers; `observe`, `differential` and `edits` are extensions
(see [Interop drivers](../../spec/porting.md#interop-drivers)). The scripts in `scripts/` reuse the shared
harness in `../swift/scripts/harness.py`. The driver reaches the unexported
state records through `internal/interopbridge`. The editing corpus is compared
with Python only (`scripts/editing_corpus.py`); the `edit` category of the shared
differential corpus is compared three ways.

## Package QA

`just check-package` runs `scripts/package_qa.py`, which copies the module's
files (`git ls-files --cached --others --exclude-standard go`, so uncommitted
work is included) to a temporary directory **outside the repository**, builds
the packaged example CLIs there, and runs a small consumer program that imports
the module, loads an embedded bundle with `LoadBundleFS`, and exercises install,
inspect and uninstall. Building outside the checkout proves the module needs
nothing from sibling directories. This is the one place Go development uses a
temporary directory outside the checkout; it is removed afterwards. QA never
publishes.

## CI and maintenance

[CI](../../.github/workflows/ci.yml) runs the Go fast job on Ubuntu 24.04 and
macOS 26 for changes under `go/`, `swift/scripts/`, and every path that selects
Rust; see the
[path selection table](../../python/docs/development.md#ci-and-maintenance).
Go tests compare against Python and Rust, so changes to those run Go too.

| Job | When | What |
| --- | --- | --- |
| `Go (os)` | Pushes, nightly, dispatch, and affected pull requests | Pins Go 1.27.3 with `GOTOOLCHAIN=local` and asserts the version, installs golangci-lint, then runs the generated-table check, formatting, `go vet`, lint, the cross-platform builds, `go test -short ./...`, the case-folding cross-check and govulncheck. |
| `Go integration (os)` | Nightly schedule and dispatch | Provisions Python, uv, Rust, Node 24.21.0, Bun 1.4.2 and a pinned `just`, and asserts that Swift is available on the macOS runner, then runs the state-vector check, `go test -race -count=1 -tags integration ./...`, the three cross-language scripts, `go mod verify` and package QA. |

Both jobs count toward `CI result`, which is the single required status check.
CI runs the underlying commands directly, not through `just`, with the same
`.cache/go` variables as the Justfile except `GOTOOLCHAIN: local`: setup-go
installs the pinned toolchain and a step asserts its version, so CI never
downloads one. The Justfile's `go1.27.3+auto` is for local runs only. Toolchain,
Node, npm, Bun and tool versions are pinned in CI and the Justfile. The GitHub
Actions lockfile records the commits for workflow action tags.
[Dependabot](../../.github/dependabot.yml) groups
weekly `gomod` updates under `/go`; the golangci-lint and govulncheck pins are
embedded tool versions updated explicitly.

## Releasing Go

The Go module is released as a tag; there is no registry upload. Go versions are
independent of the other implementations, and tags are `go/vX.Y.Z`, which is the
form the Go module proxy reads for a module in the `go/` subdirectory, so
`go get github.com/nielsmadan/flyrail/go/flyrail@vX.Y.Z` resolves it.
[`release-go.yml`](../../.github/workflows/release-go.yml) validates and
publishes the tag. Releases use
[`curtains`](https://github.com/nielsmadan/curtaincall) from a clean, complete
`main` checkout at the repository root, driven by
[`scripts/release.json`](../scripts/release.json):

```sh
just release go --dry-run
just release go
just release go patch
just release go 0.2.0
```

The dry run needs a clean, committed working tree and SSH access to the
repository remote. Curtains considers commits touching `go/`, runs the Go
`just check`, `just check-integration`, `just audit-dependencies` and the
workflow validation, writes
[`CHANGELOG.md`](../CHANGELOG.md) with git-cliff, runs package QA against the
prepared version, then signs the commit and annotated tag and atomically pushes `main` with the tag.
Creating a release commit or tag, pushing, and publishing need explicit
authorization from the repository owner.

The workflow validates the annotated tag and `go.mod`, reruns the full fast and
integration gates, govulncheck and package QA on macOS, and saves the verified release
notes. A `contents: write` job then creates the "Flyrail for Go" GitHub release;
versions below 1.0 are marked pre-release. If publication fails after the tag is
pushed, fix the workflow on `main` and rerun it from the Actions page with
**Run workflow** on `main`, supplying the existing tag, or with the GitHub CLI:

```sh
gh workflow run release-go.yml --ref main -f tag=go/v0.1.0
```

Never move a published tag; the Go module proxy caches tagged versions
permanently.
