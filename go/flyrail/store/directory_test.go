//go:build darwin || linux

package store_test

import (
	"context"
	"errors"
	"io/fs"
	"os"
	"path/filepath"
	"syscall"
	"testing"
	"time"

	"github.com/nielsmadan/flyrail/go/flyrail/store"
	"github.com/nielsmadan/flyrail/go/flyrail/store/storetest"
)

func tempRoot(t *testing.T) string {
	t.Helper()
	root, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	return root
}

func directoryStore(t *testing.T, path string) *store.DirectoryStore {
	t.Helper()
	s, err := store.NewDirectoryStore(t.Context(), path)
	if err != nil {
		t.Fatal(err)
	}
	return s
}

func mode(t *testing.T, path string) fs.FileMode {
	t.Helper()
	info, err := os.Lstat(path)
	if err != nil {
		t.Fatal(err)
	}
	return info.Mode().Perm()
}

func TestDirectoryStoreMeetsTheContract(t *testing.T) {
	t.Parallel()
	path := filepath.Join(tempRoot(t), "state")
	storetest.Run(t, func(t *testing.T) store.Store { return directoryStore(t, path) })
}

func TestDirectoryStoreKeepsPrivateRecordsInTheSharedLayout(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	s := directoryStore(t, filepath.Join(root, "state"))
	written, err := s.Write(t.Context(), "index/app/x", []byte("r"), store.Absent())
	if err != nil || !written {
		t.Fatalf("Write = %v, %v", written, err)
	}
	held, err := s.Lock(t.Context(), "resource/abc", 0)
	if err != nil {
		t.Fatal(err)
	}
	if err := held.Unlock(); err != nil {
		t.Fatal(err)
	}
	for path, want := range map[string]fs.FileMode{
		"state":                     0o700,
		"state/records":             0o700,
		"state/records/index~app~x": 0o600,
		"state/locks":               0o700,
		"state/locks/resource~abc":  0o600,
		"state/mutation":            0o600,
	} {
		if got := mode(t, filepath.Join(root, path)); got != want {
			t.Errorf("mode of %s = %v, want %v", path, got, want)
		}
	}
	entries, err := os.ReadDir(filepath.Join(root, "state/records"))
	if err != nil {
		t.Fatal(err)
	}
	if len(entries) != 1 || entries[0].Name() != "index~app~x" {
		t.Errorf("records = %v, want only index~app~x", entries)
	}
	data, err := os.ReadFile(filepath.Join(root, "state/records/index~app~x"))
	if err != nil || string(data) != "r" {
		t.Errorf("record = %q, %v", data, err)
	}
}

func TestDirectoryStoreRefusesAPublicRoot(t *testing.T) {
	t.Parallel()
	public := filepath.Join(tempRoot(t), "public")
	if err := os.Mkdir(public, 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.Chmod(public, 0o777); err != nil {
		t.Fatal(err)
	}
	_, _, err := directoryStore(t, public).Read(t.Context(), "index/app/x")
	failure, ok := errors.AsType[*store.IOError](err)
	if !ok {
		t.Fatalf("Read error = %v, want *store.IOError", err)
	}
	want := "[Errno 13] state store must be private to its owner: '" + public + "'"
	if failure.Message != want || failure.Error() != want || failure.Path != public || failure.Errno != syscall.EACCES {
		t.Errorf("IOError = %+v, want message %q", failure, want)
	}
	if !errors.Is(err, syscall.EACCES) {
		t.Errorf("errors.Is(%v, EACCES) = false", err)
	}
	if pathErr, ok := errors.AsType[*fs.PathError](err); !ok || pathErr.Path != public {
		t.Errorf("Unwrap = %v, want *fs.PathError for %s", errors.Unwrap(err), public)
	}
}

func TestDirectoryStoreReportsIOFailuresWithPythonText(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	path := filepath.Join(root, "state")
	s := directoryStore(t, path)
	if _, err := s.Write(t.Context(), "index/a", []byte("x"), store.Absent()); err != nil {
		t.Fatal(err)
	}
	record := filepath.Join(path, "records", "index~b")
	if err := os.Mkdir(record, 0o700); err != nil {
		t.Fatal(err)
	}
	_, _, err := s.Read(t.Context(), "index/b")
	failure, ok := errors.AsType[*store.IOError](err)
	want := "[Errno 21] Is a directory: '" + record + "'"
	if !ok || failure.Message != want || failure.Errno != syscall.EISDIR || failure.Path != record {
		t.Errorf("Read error = %#v, want %q", err, want)
	}
}

func TestDirectoryStoreLocationAndStaging(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	s := directoryStore(t, filepath.Join(root, "state"))
	if location, ok := s.Location(); !ok || location != filepath.Join(root, "state") {
		t.Errorf("Location = %q, %v", location, ok)
	}
	if staging, ok := s.StagingDirectory(); !ok || staging != filepath.Join(root, "state/staging") {
		t.Errorf("StagingDirectory = %q, %v", staging, ok)
	}
	memory := store.NewMemoryStore()
	location, hasLocation := memory.Location()
	staging, hasStaging := memory.StagingDirectory()
	if location != "" || hasLocation || staging != "" || hasStaging {
		t.Errorf("memory store location = %q %v, staging = %q %v", location, hasLocation, staging, hasStaging)
	}
}

func TestDirectoryStoreLocationIsMadeAbsoluteLikeRust(t *testing.T) {
	t.Parallel()
	current, err := os.Getwd()
	if err != nil {
		t.Fatal(err)
	}
	for _, test := range []struct{ path, want string }{
		{"/a//b/./c", "/a/b/c"},
		{"/a/../b/", "/a/../b/"},
		{"//net/x", "//net/x"},
		{"///x", "/x"},
		{"rel/./x", current + "/rel/x"},
		{"./", current + "/"},
	} {
		if location, _ := directoryStore(t, test.path).Location(); location != test.want {
			t.Errorf("Location(%q) = %q, want %q", test.path, location, test.want)
		}
	}
	_, err = store.NewDirectoryStore(t.Context(), "")
	if failure, ok := errors.AsType[*store.IOError](err); !ok || failure.Message != "cannot make an empty path absolute" {
		t.Errorf("NewDirectoryStore(\"\") error = %v", err)
	}
}

// Not parallel: a re-exec test's forked child can briefly hold the closed mutation lock's inherited description.
func TestDirectoryStoreMutationLockHonoursCancellation(t *testing.T) {
	path := filepath.Join(tempRoot(t), "state")
	first, second := directoryStore(t, path), directoryStore(t, path)
	if _, err := first.Write(t.Context(), "index/a", []byte("x"), store.Absent()); err != nil {
		t.Fatal(err)
	}
	held, err := first.Lock(t.Context(), "probe", 0)
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = held.Unlock() }()
	mutation, err := os.OpenFile(filepath.Join(path, "mutation"), os.O_RDWR, 0)
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = mutation.Close() }()
	if err := syscall.Flock(int(mutation.Fd()), syscall.LOCK_EX|syscall.LOCK_NB); err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithTimeout(t.Context(), 100*time.Millisecond)
	defer cancel()
	started := time.Now()
	written, err := second.Write(ctx, "index/a", []byte("y"), store.Equal([]byte("x")))
	busy, ok := errors.AsType[*store.BusyError](err)
	if written || !ok || busy.Name != "mutation" || !errors.Is(err, context.DeadlineExceeded) {
		t.Fatalf("Write = %v, %v; want a busy mutation lock wrapping the deadline", written, err)
	}
	if elapsed := time.Since(started); elapsed < 50*time.Millisecond {
		t.Errorf("Write gave up after %v", elapsed)
	}
}

func TestDirectoryStoreRefusesValuesOverTheReadCap(t *testing.T) {
	t.Parallel()
	root := filepath.Join(tempRoot(t), "state")
	s := directoryStore(t, root).WithRecordLimit(8)
	record := filepath.Join(root, "records", "cap")
	_, err := s.Write(t.Context(), "cap", []byte("123456789"), store.Absent())
	want := "[Errno 27] File too large: '" + record + "'"
	if ioErr, ok := errors.AsType[*store.IOError](err); !ok || ioErr.Error() != want || !errors.Is(err, syscall.EFBIG) {
		t.Fatalf("Write over the cap = %v, want %q", err, want)
	}
	if _, err := os.Lstat(root); !errors.Is(err, fs.ErrNotExist) {
		t.Errorf("a refused write created %s: %v", root, err)
	}
	if won, err := s.Write(t.Context(), "cap", []byte("12345678"), store.Absent()); err != nil || !won {
		t.Fatalf("Write at the cap = %v, %v", won, err)
	}
	if value, ok, err := s.Read(t.Context(), "cap"); err != nil || !equalBytes(value, ok, "12345678") {
		t.Errorf("Read at the cap = %q, %v, %v", value, ok, err)
	}
}

func TestDirectoryStoreRefusesToReadRecordsOverTheCap(t *testing.T) {
	t.Parallel()
	root := filepath.Join(tempRoot(t), "state")
	s := directoryStore(t, root)
	if won, err := s.Write(t.Context(), "cap", []byte("123456789"), store.Absent()); err != nil || !won {
		t.Fatalf("Write = %v, %v", won, err)
	}
	_, _, err := s.WithRecordLimit(8).Read(t.Context(), "cap")
	want := "[Errno 27] File too large: '" + filepath.Join(root, "records", "cap") + "'"
	if ioErr, ok := errors.AsType[*store.IOError](err); !ok || ioErr.Error() != want || !errors.Is(err, syscall.EFBIG) {
		t.Fatalf("Read over the cap = %v, want %q", err, want)
	}
}

func equalBytes(value []byte, ok bool, want string) bool {
	return ok && string(value) == want
}
