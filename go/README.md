# Flyrail for Go

Flyrail is a Go library for reversible skills, instructions, MCP registrations
and hooks at explicit local destinations. A Go CLI embeds it to install its own
agent configuration, report whether that configuration is current, and remove
it again without touching content it does not own.

Flyrail snapshots a bundle, renders it for an agent you name, and applies it
through per-resource ownership records kept in a state store you choose. It
never prints, prompts, spawns processes, reads environment variables, uses the
network or picks an installed agent on its own.

The module is `github.com/nielsmadan/flyrail/go` and needs Go 1.27 or later on
macOS or Linux. Once released it is consumed through a `go/vX.Y.Z` tag with
`go get github.com/nielsmadan/flyrail/go/flyrail`; until the first release,
`go get github.com/nielsmadan/flyrail/go/flyrail@main` resolves a pseudo-version.

The library needs no cgo, so a CLI that imports it still cross-compiles with
`CGO_ENABLED=0`. Other operating systems compile but report every filesystem
operation as unsupported; see [platform support](docs/support.md).

## Quickstart

Start a module, add Flyrail, and run the program below. Until the first
`go/vX.Y.Z` release is tagged, `go get` resolves the module's `main` branch as a
pseudo-version; afterwards it resolves the latest tag.

```sh
mkdir mycli && cd mycli
go mod init example.com/mycli
go get github.com/nielsmadan/flyrail/go/flyrail@main
```

A CLI ships its skills inside the binary, loads them as a bundle, and installs
them into a skill directory. Every call that touches the filesystem takes a
`context.Context` first. Lifecycle calls report what happened to each target in
the returned results; `Succeeded()` and `Failure()` turn a result into a boolean
or an `error` without the typed-nil trap that returning `result.Err` directly
would set.

Put the following in `main.go`. `skillLifecycle` installs a skill, inspects it,
and uninstalls it. `renderAndSync` builds a bundle in memory, previews the
change, writes an `AGENTS.md` section for Codex, and removes it: instructions,
MCP registrations and hooks go through an explicit render step because their
destinations depend on the agent and the project. `exampleDirectory` only makes
a scratch directory.

```go
package main

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"testing/fstest"
	"time"

	"github.com/nielsmadan/flyrail/go/flyrail"
	"github.com/nielsmadan/flyrail/go/flyrail/store"
)

func main() {
	if err := run(context.Background()); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}

func run(ctx context.Context) error {
	if err := skillLifecycle(ctx); err != nil {
		return err
	}
	return renderAndSync(ctx)
}

func exampleDirectory() (string, func(), error) {
	created, err := os.MkdirTemp("", "flyrail-example-")
	if err != nil {
		return "", nil, err
	}
	cleanup := func() { _ = os.RemoveAll(created) }
	resolved, err := filepath.EvalSymlinks(created)
	if err != nil {
		cleanup()
		return "", nil, err
	}
	return resolved, cleanup, nil
}

// A CLI normally ships its skills in an embed.FS; an fstest.MapFS stands in for one here.
func skillLifecycle(ctx context.Context) error {
	dir, cleanup, err := exampleDirectory()
	if err != nil {
		return err
	}
	defer cleanup()
	st, err := store.NewDirectoryStore(ctx, filepath.Join(dir, "state"))
	if err != nil {
		return err
	}
	embedded := fstest.MapFS{
		"flyrail.json":    {Data: []byte(`{"schema_version": 1, "id": "team", "version": "1", "skills": [{"name": "review", "path": "review"}]}`)},
		"review/SKILL.md": {Data: []byte("---\nname: review\ndescription: Review a change.\n---\nCheck it.\n")},
	}
	bundle, err := flyrail.LoadBundleFS(embedded)
	if err != nil {
		return err
	}
	target, err := flyrail.DirectoryTarget(filepath.Join(dir, "skills"))
	if err != nil {
		return err
	}
	targets := []flyrail.Target{target}

	installed, err := flyrail.Install(ctx, bundle, targets, st, flyrail.LockOptions{LockTimeout: time.Second})
	if err != nil {
		return err
	}
	for _, result := range installed {
		if !result.Succeeded() {
			return result.Failure()
		}
		fmt.Println("install:", result.Status)
	}

	inspected, err := flyrail.Inspect(ctx, bundle, targets, st)
	if err != nil {
		return err
	}
	observation := inspected[0].Observation
	fmt.Printf("inspect: %s current=%t\n", observation.State, observation.IsCurrent())
	if observation.Installed != nil {
		for _, entry := range observation.Installed.Entries {
			kind := "file"
			if entry.Dir {
				kind = "dir"
			}
			fmt.Printf("  %s %d %s\n", entry.Path, entry.Size, kind)
		}
	}

	removed, err := flyrail.Uninstall(ctx, bundle.ID(), targets, st, flyrail.SkillOptions{LockTimeout: time.Second})
	if err != nil {
		return err
	}
	if err := removed[0].Failure(); err != nil {
		return err
	}
	fmt.Println("uninstall:", removed[0].Status)

	inspected, err = flyrail.Inspect(ctx, bundle, targets, st)
	if err != nil {
		return err
	}
	fmt.Println("inspect:", inspected[0].Observation.State)
	return nil
}

func renderAndSync(ctx context.Context) error {
	dir, cleanup, err := exampleDirectory()
	if err != nil {
		return err
	}
	defer cleanup()
	project := filepath.Join(dir, "project")
	if err := os.Mkdir(project, 0o700); err != nil {
		return err
	}
	st, err := store.NewDirectoryStore(ctx, filepath.Join(dir, "state"))
	if err != nil {
		return err
	}
	identity, err := flyrail.NewBundleIdentity("guidance", "1")
	if err != nil {
		return err
	}
	instruction, err := flyrail.NewInstructionArtifact("style", "Prefer small, reviewed changes.\n")
	if err != nil {
		return err
	}
	bundle, err := flyrail.NewBundle(identity, []flyrail.Artifact{instruction}, nil, nil)
	if err != nil {
		return err
	}
	target, err := flyrail.ProjectTarget(flyrail.AgentCodex, project)
	if err != nil {
		return err
	}
	platform := flyrail.PlatformLinux
	if runtime.GOOS == "darwin" {
		platform = flyrail.PlatformMacOS
	}
	rc, err := flyrail.NewRenderContext(target, platform, flyrail.RenderOptions{})
	if err != nil {
		return err
	}
	rendered, err := flyrail.Render(bundle, rc)
	if err != nil {
		return err
	}
	installation, err := rc.Installation(st, nil)
	if err != nil {
		return err
	}

	preview, err := flyrail.Preview(ctx, bundle, rendered, installation, flyrail.PreviewOptions{})
	if err != nil {
		return err
	}
	summary, err := flyrail.Summarize(preview)
	if err != nil {
		return err
	}
	for _, change := range summary.Changes {
		fmt.Printf("plan: %s %s %v\n", change.Action, filepath.Base(change.Destination), change.ArtifactIDs)
	}

	result, err := flyrail.Sync(ctx, bundle, rendered, installation, flyrail.SyncOptions{LockTimeout: time.Second})
	if err != nil {
		return err
	}
	if err := result.Failure(); err != nil {
		return err
	}
	fmt.Println("sync:", result.Status)
	written, err := os.ReadFile(filepath.Join(project, "AGENTS.md"))
	if err != nil {
		return err
	}
	fmt.Print(string(written))

	removed, err := flyrail.Remove(ctx, bundle.ID(), installation, flyrail.RemoveOptions{LockTimeout: time.Second})
	if err != nil {
		return err
	}
	if err := removed.Failure(); err != nil {
		return err
	}
	fmt.Println("remove:", removed.Status)
	_, err = os.Stat(filepath.Join(project, "AGENTS.md"))
	fmt.Println("AGENTS.md exists:", err == nil)
	return nil
}
```

Then run it:

```sh
go run .
```

It prints:

```text
install: applied
inspect: installed current=true
  review 0 dir
  review/SKILL.md 61 file
uninstall: applied
inspect: absent
plan: create AGENTS.md [style]
sync: applied
<!-- flyrail:d8fef1fafc5c66dd4c36a87125bf9baadd4de514055d68683e091c487cbdb9f3:start -->
Prefer small, reviewed changes.
<!-- flyrail:d8fef1fafc5c66dd4c36a87125bf9baadd4de514055d68683e091c487cbdb9f3:end -->
remove: applied
AGENTS.md exists: false
```

The two functions are the `Example` functions in [`flyrail/example_test.go`](flyrail/example_test.go), which `go test` runs and checks against the output shown.

## Embedding a bundle

In a real CLI the bundle comes from your binary. Put a directory that holds
`flyrail.json` and the skill folders next to the Go file that embeds it (here
`bundle/`), embed it, and load it with `LoadBundleFS`. `LoadBundleFS` reads
`flyrail.json` at the root of the file system it is given, so strip the
directory prefix with `fs.Sub`. The `all:` prefix also embeds files whose names
start with `.` or `_`, which `//go:embed` otherwise skips:

```go
import (
	"embed"
	"io/fs"

	"github.com/nielsmadan/flyrail/go/flyrail"
)

//go:embed all:bundle
var embedded embed.FS

func loadBundle() (*flyrail.Bundle, error) {
	root, err := fs.Sub(embedded, "bundle")
	if err != nil {
		return nil, err
	}
	return flyrail.LoadBundleFS(root)
}
```

The state store is the CLI's own: choose a directory under your application's
state location and pass the same store to every call. Flyrail has no default
store.

## Global Loadout

A host can discover an existing global Loadout setup and offer it as an
installation destination. The user chooses a declared source and reviews the
planned edits. After applying, the host reports when the user needs to run
Loadout sync. See the [Go integration example](docs/examples.md#global-loadout).

## Documentation

- [Public API](docs/api.md): packages, conventions and the Rust-to-Go crosswalk of every exported item.
- [Lifecycle and recovery](docs/lifecycle.md): previews, sync, removal, results and contexts.
- [Configuration and document editing](docs/configuration.md): bundles, content and edits.
- [Translation and destinations](docs/translation.md): direct rendering and Loadout source mappings.
- [Inspection](docs/inspection.md): skill destinations and read-only observation.
- [Bundled hooks](docs/hooks.md): the hook runtime snapshot and protocol values.
- [Architecture](docs/architecture.md): packages, boundaries and the no-cgo native layer.
- [Examples](docs/examples.md): the two example CLIs.
- [Platform support](docs/support.md): supported systems, refusals and resource limits.
- [Development](docs/development.md): commands, caches, CI and releases.

The shared formats and conformance fixtures are in [`../spec`](../spec/README.md).

## Development

Run Go only through `just` from this directory: `just check` is the fast gate,
`just check-integration` the full tier, and `just check-package` verifies the
module as a consumer sees it. [Development](docs/development.md) describes the
main recipes; `go/Justfile` lists them all, and `just --list` prints them.
