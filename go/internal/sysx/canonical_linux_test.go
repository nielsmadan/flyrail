package sysx

import (
	"os"
	"path/filepath"
	"testing"
)

func TestCanonicalizeFallsBackToPhysicalResolutionWithoutProc(t *testing.T) {
	root := tempRoot(t)
	if err := os.MkdirAll(filepath.Join(root, "real", "inner"), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink("real", filepath.Join(root, "alias")); err != nil {
		t.Fatal(err)
	}
	path := filepath.Join(root, "alias", "inner", "..", "inner")
	got, err := canonicalizeVia(path, filepath.Join(root, "no-proc"))
	want := filepath.Join(root, "real", "inner")
	if err != nil || got != want {
		t.Fatalf("canonicalizeVia without procfs = %q, %v, want %q", got, err, want)
	}
	viaProc, err := Canonicalize(path)
	if err != nil || viaProc != want {
		t.Fatalf("Canonicalize = %q, %v, want %q", viaProc, err, want)
	}
}

func TestCanonicalizeWithoutProcResolvesTheRoot(t *testing.T) {
	got, err := canonicalizeVia("/", filepath.Join(tempRoot(t), "no-proc"))
	if err != nil || got != "/" {
		t.Fatalf("canonicalizeVia(/) = %q, %v, want /", got, err)
	}
}
