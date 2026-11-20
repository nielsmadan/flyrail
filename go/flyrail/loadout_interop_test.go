package flyrail

import (
	"os"
	"path/filepath"
	"runtime"
	"testing"

	"github.com/nielsmadan/flyrail/go/flyrail/store"
)

func TestLoadoutInteropProducer(t *testing.T) {
	root := os.Getenv("FLYRAIL_LOADOUT_INTEROP_ROOT")
	if root == "" {
		t.Skip("run spec/check_loadout_interop.py to provision the fixture")
	}
	ctx := t.Context()
	state, err := store.NewDirectoryStore(ctx, filepath.Join(root, "state"))
	if err != nil {
		t.Fatal(err)
	}
	switch os.Getenv("FLYRAIL_LOADOUT_INTEROP_PHASE") {
	case "install":
		hooks, err := NewHookRuntime(os.Getenv("FLYRAIL_LOADOUT_INTEROP_NODE"), ShellPOSIX)
		if err != nil {
			t.Fatal(err)
		}
		platform := PlatformLinux
		if runtime.GOOS == "darwin" {
			platform = PlatformMacOS
		}
		options := LoadoutOptions{Platform: platform, HookRuntime: hooks, ExecutionRoot: root}
		for _, bundleID := range []string{"interop", "extra"} {
			bundle, err := OpenBundle(ctx, filepath.Join(root, bundleID))
			if err != nil {
				t.Fatal(err)
			}
			agents := []Agent{AgentClaude, AgentCodex, AgentCursor, AgentPi}
			if bundleID == "interop" {
				agents = append(agents, AgentOpencode)
			}
			for _, expected := range []OperationStatus{StatusApplied, StatusUnchanged} {
				found, err := DiscoverLoadout(filepath.Join(root, "home"), "", nil)
				if err != nil || found.Setup == nil {
					t.Fatalf("discover: %+v %v", found, err)
				}
				destination, err := found.Setup.Select("local", agents, false)
				if err != nil {
					t.Fatal(err)
				}
				plan, err := PreviewLoadout(ctx, bundle, destination, state, options)
				if err != nil || !plan.Applicable() {
					t.Fatalf("preview %s: %v %+v", bundleID, err, plan)
				}
				result := ApplyLoadout(ctx, plan, LockOptions{})
				if result.Lifecycle.Status != expected || expected == StatusApplied && !result.SyncRequired() {
					t.Fatalf("apply %s, expected %s: %+v", bundleID, expected, result.Lifecycle)
				}
				observation, err := InspectLoadout(ctx, bundleID, plan.Location(), state)
				if err != nil || !observation.Current {
					t.Fatalf("inspect %s: %+v %v", bundleID, observation, err)
				}
			}
		}
	case "remove":
		source := os.Getenv("FLYRAIL_LOADOUT_INTEROP_SOURCE")
		location, err := NewLoadoutLocation(source, "default", "local", source)
		if err != nil {
			t.Fatal(err)
		}
		for _, bundleID := range []string{"extra", "interop"} {
			observation, err := InspectLoadout(ctx, bundleID, location, state)
			if err != nil || !observation.Current {
				t.Fatalf("inspect %s: %+v %v", bundleID, observation, err)
			}
			plan, err := PreviewLoadoutRemoval(ctx, bundleID, location, state, false)
			if err != nil || !plan.Applicable() {
				t.Fatalf("preview removal %s: %v %+v", bundleID, err, plan)
			}
			result := ApplyLoadout(ctx, plan, LockOptions{})
			if result.Lifecycle.Status != StatusApplied || !result.SyncRequired() {
				t.Fatalf("remove %s: %+v", bundleID, result.Lifecycle)
			}
		}
	default:
		t.Fatal("expected install or remove phase")
	}
}
