package sysx

import (
	"path/filepath"
	"testing"
)

func TestReadMountInfoReturnsTheTableUpToTheLimit(t *testing.T) {
	path := filepath.Join(tempRoot(t), "mountinfo")
	writeFile(t, path, "0123456789")
	if got := readMountInfo(path, 10); string(got) != "0123456789" {
		t.Fatalf("readMountInfo at the limit = %q, want the table", got)
	}
}

func TestReadMountInfoReturnsNothingBeyondTheLimit(t *testing.T) {
	path := filepath.Join(tempRoot(t), "mountinfo")
	writeFile(t, path, "0123456789A")
	if got := readMountInfo(path, 10); got != nil {
		t.Fatalf("readMountInfo past the limit = %q, want nil", got)
	}
}

func TestReadMountInfoReturnsNothingForAMissingFile(t *testing.T) {
	if got := readMountInfo(filepath.Join(tempRoot(t), "absent"), 10); got != nil {
		t.Fatalf("readMountInfo of a missing file = %q, want nil", got)
	}
}
