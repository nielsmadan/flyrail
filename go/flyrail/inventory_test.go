package flyrail

import (
	"testing"

	"github.com/google/go-cmp/cmp"
)

func TestInventorySortsPathsAndHashesFileBytes(t *testing.T) {
	t.Parallel()
	values := must(inventory([]BundleEntry{
		{Path: "z", Data: []byte("last")},
		{Path: "a", Dir: true},
		{Path: "a/run", Data: []byte("run"), Executable: true},
		{Path: "empty", Data: []byte{}},
	}))
	want := []InventoryEntry{
		{Path: "a", Dir: true},
		{Path: "a/run", Size: 3, SHA256: "acba25512100f80b56fc3ccd14c65be55d94800cda77585c5f41a887e398f9be", Executable: true},
		{Path: "empty", SHA256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"},
		{Path: "z", Size: 4, SHA256: "3547cb112ac4489af2310c0626cdba6f3097a2ad5a3b42ddd3b59c76c7a079a3"},
	}
	if diff := cmp.Diff(want, values); diff != "" {
		t.Errorf("inventory (-want +got):\n%s", diff)
	}
}

func TestInventoryPathsAreStrictlyValidated(t *testing.T) {
	t.Parallel()
	if message := valueMessage(t, errorOf(inventory([]BundleEntry{{Path: "../escape", Dir: true}}))); message != "inventory path must be an unambiguous relative path" {
		t.Errorf("message = %q", message)
	}
}
