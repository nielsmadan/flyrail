# Flyrail for Go

Flyrail is a Go library for reversible skills, instructions, MCP registrations
and hooks at explicit local destinations. Keep dependencies small and public
values immutable. Library calls must never print, prompt, exit, spawn
processes, read environment variables implicitly, make network requests, or
select installed agents.

Read README.md and the relevant docs before changing an interface. Keep
`docs/api.md`, the lifecycle, translation and example docs consistent with
behavioural changes, and keep the README quickstart identical to the `Example`
functions in `flyrail/example_test.go`. Rust is the behavioural reference;
durable formats and cross-language error text must match it byte for byte,
except TOML and document editing output, which follows Python's tomlkit, and
resource caps and hardening, which follow the decisions recorded in docs/support.md.
`store` imports nothing from `flyrail`, and `internal/*` imports neither public
package; `flyrail` holds all types and logic. State records stay unexported.
Test seams travel in the context (`internal/seams`), never in build tags or
package-level state. Every exported identifier has a one-sentence doc comment; otherwise prefer clear
names over comments.

Run Go only through `just` from this directory; the Justfile sets the
repository-local environment. `just check` runs formatting, `go vet`,
golangci-lint, the other-platform builds and the short test tier; it is the
git-hook gate and must stay fast. `just check-integration` runs the
race-enabled integration tier and the cross-language checks, `just check-package`
verifies the module as a consumer sees it, and `just check-linux` runs the short
tier in the `golang:1.27.3` container (`check-linux-integration` the full tier).
`just format` formats Go code, `just lint` runs golangci-lint, and `just test`
runs the whole suite. `just release go --dry-run` (from the repository root)
previews a Go release; creating a release commit or tag, pushing, and
publishing require explicit user authorization. A pushed `go/vX.Y.Z` tag is the
release, so it is never moved. Supported platforms are macOS and Linux; other
systems build but report unsupported operations.

The library builds without cgo. libSystem calls that golang.org/x/sys lacks go
through the trampolines in `internal/darwin`; every raw system call goes
through the EINTR retry in `internal/sysx`. Never export `CGO_ENABLED=0`
globally: the race detector needs cgo.

Keep caches, temporary files, downloaded tools, and test installations inside
this checkout; the Justfile points GOPATH, GOMODCACHE, GOCACHE and the
golangci-lint cache at the repository-root `.cache/go`, and `scripts/tools.py`
downloads checksum-verified tools into `.cache/tools`. The one exception is
`scripts/package_qa.py`, which builds a copy of the module in a temporary
directory outside the repository on purpose and removes it afterwards. Tests
must use temporary roots and explicit environment overrides, never the real
user's agent configuration. Do not change HOME, activate hooks, or install
global tools.

Validate public input behavior and observable filesystem outcomes with
positive assertions. Run the complete check and read its exit status and full
test summary before reporting.

Shared conformance fixtures live in `../spec/fixtures`; when the fixture
directory is absent, conformance tests skip with a notice unless
`FLYRAIL_REQUIRE_FIXTURES=1`, which the Justfile sets.

Payload and metadata writes use exclusive create (`O_CREATE|O_EXCL`); never use
a truncating open or `os.WriteFile`, because they destroy recovery evidence.
Resources are published with an exclusive rename. Library code never panics
on public paths.

Hook runtime checks require Node, npm and Bun; CI pins Node 24.21.0, npm 11.19.0
and Bun 1.4.2.
