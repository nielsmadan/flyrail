//go:build darwin || linux

package flyrail_test

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
func Example_skillLifecycle() {
	if err := skillLifecycle(context.Background()); err != nil {
		fmt.Println("error:", err)
	}
	// Output:
	// install: applied
	// inspect: installed current=true
	//   review 0 dir
	//   review/SKILL.md 61 file
	// uninstall: applied
	// inspect: absent
}

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

func Example_renderAndSync() {
	if err := renderAndSync(context.Background()); err != nil {
		fmt.Println("error:", err)
	}
	// Output:
	// plan: create AGENTS.md [style]
	// sync: applied
	// <!-- flyrail:d8fef1fafc5c66dd4c36a87125bf9baadd4de514055d68683e091c487cbdb9f3:start -->
	// Prefer small, reviewed changes.
	// <!-- flyrail:d8fef1fafc5c66dd4c36a87125bf9baadd4de514055d68683e091c487cbdb9f3:end -->
	// remove: applied
	// AGENTS.md exists: false
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
