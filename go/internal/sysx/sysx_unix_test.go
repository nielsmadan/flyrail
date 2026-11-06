//go:build darwin || linux

package sysx

import (
	"errors"
	"io"
	"io/fs"
	"os"
	"path/filepath"
	"strings"
	"syscall"
	"testing"

	"golang.org/x/sys/unix"
)

func tempRoot(t *testing.T) string {
	t.Helper()
	root, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	return root
}

func writeFile(t *testing.T, path, content string) {
	t.Helper()
	if err := os.WriteFile(path, []byte(content), 0o600); err != nil {
		t.Fatal(err)
	}
}

func readFile(t *testing.T, path string) string {
	t.Helper()
	content, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	return string(content)
}

func TestRenameExclusivePublishesToAbsentDestination(t *testing.T) {
	root := tempRoot(t)
	from, to := filepath.Join(root, "staged"), filepath.Join(root, "published")
	writeFile(t, from, "payload")
	if err := RenameExclusive(from, to); err != nil {
		t.Fatalf("RenameExclusive: %v", err)
	}
	if got := readFile(t, to); got != "payload" {
		t.Fatalf("published = %q, want payload", got)
	}
}

func TestRenameExclusiveRefusesExistingDestination(t *testing.T) {
	root := tempRoot(t)
	from, to := filepath.Join(root, "staged"), filepath.Join(root, "existing")
	writeFile(t, from, "new")
	writeFile(t, to, "evidence")
	err := RenameExclusive(from, to)
	var linkErr *os.LinkError
	if !errors.As(err, &linkErr) || !errors.Is(err, syscall.EEXIST) {
		t.Fatalf("RenameExclusive error = %v, want *os.LinkError with EEXIST", err)
	}
	if got := readFile(t, to); got != "evidence" {
		t.Fatalf("destination = %q, want evidence", got)
	}
	if got := readFile(t, from); got != "new" {
		t.Fatalf("source = %q, want new", got)
	}
}

func TestRenameatExclusiveRefusesExistingDestination(t *testing.T) {
	root := tempRoot(t)
	writeFile(t, filepath.Join(root, "staged"), "new")
	writeFile(t, filepath.Join(root, "existing"), "evidence")
	dir, err := os.Open(root)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		if err := dir.Close(); err != nil {
			t.Error(err)
		}
	})
	fd := int(dir.Fd())
	if err := RenameatExclusive(fd, "staged", fd, "existing"); !errors.Is(err, syscall.EEXIST) {
		t.Fatalf("RenameatExclusive error = %v, want EEXIST", err)
	}
	if err := RenameatExclusive(fd, "staged", fd, "published"); err != nil {
		t.Fatalf("RenameatExclusive: %v", err)
	}
	if got := readFile(t, filepath.Join(root, "published")); got != "new" {
		t.Fatalf("published = %q, want new", got)
	}
}

func TestFileLockExcludesSecondHandleUntilUnlock(t *testing.T) {
	path := filepath.Join(tempRoot(t), "lock")
	first, err := OpenLockFile(path)
	if err != nil {
		t.Fatal(err)
	}
	second, err := OpenLockFile(path)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		if err := second.Unlock(); err != nil {
			t.Error(err)
		}
	})
	if acquired, err := first.TryLock(); !acquired || err != nil {
		t.Fatalf("first TryLock = %v, %v, want true, nil", acquired, err)
	}
	if acquired, err := second.TryLock(); acquired || err != nil {
		t.Fatalf("second TryLock while held = %v, %v, want false, nil", acquired, err)
	}
	if err := first.Unlock(); err != nil {
		t.Fatalf("Unlock: %v", err)
	}
	if acquired, err := second.TryLock(); !acquired || err != nil {
		t.Fatalf("second TryLock after unlock = %v, %v, want true, nil", acquired, err)
	}
	info, err := os.Lstat(path)
	if err != nil || info.Mode().Perm() != 0o600 {
		t.Fatalf("lock file after unlock = %v, %v, want a 0600 file", info, err)
	}
}

func TestFileLockUnlockIsIdempotent(t *testing.T) {
	lock, err := OpenLockFile(filepath.Join(tempRoot(t), "lock"))
	if err != nil {
		t.Fatal(err)
	}
	if acquired, err := lock.TryLock(); !acquired || err != nil {
		t.Fatalf("TryLock = %v, %v, want true, nil", acquired, err)
	}
	for i := range 3 {
		if err := lock.Unlock(); err != nil {
			t.Fatalf("Unlock call %d: %v", i+1, err)
		}
	}
}

func TestOpenLockFileRefusesSymbolicLink(t *testing.T) {
	root := tempRoot(t)
	target, link := filepath.Join(root, "target"), filepath.Join(root, "lock")
	writeFile(t, target, "")
	if err := os.Symlink(target, link); err != nil {
		t.Fatal(err)
	}
	_, err := OpenLockFile(link)
	if !errors.Is(err, unix.ELOOP) {
		t.Fatalf("OpenLockFile error = %v, want ELOOP", err)
	}
}

func TestOpenNoFollowRefusesLinksAndDoesNotBlockOnFIFOs(t *testing.T) {
	root := tempRoot(t)
	target := filepath.Join(root, "target")
	writeFile(t, target, "content")
	link := filepath.Join(root, "link")
	if err := os.Symlink(target, link); err != nil {
		t.Fatal(err)
	}
	if _, err := OpenNoFollow(link); !errors.Is(err, syscall.ELOOP) {
		t.Fatalf("symlink open: %v", err)
	}
	pipe := filepath.Join(root, "pipe")
	if err := unix.Mkfifo(pipe, 0o600); err != nil {
		t.Fatal(err)
	}
	fd, err := OpenNoFollow(pipe)
	if err != nil {
		t.Fatalf("fifo open: %v", err)
	}
	signature, err := Fstat(fd, pipe)
	if err != nil || signature.IsRegular() || signature.IsDir() {
		t.Fatalf("fifo signature %+v, %v", signature, err)
	}
	if err := Close(fd); err != nil {
		t.Fatal(err)
	}
}

func TestReadAllStopsOneByteAfterTheLimit(t *testing.T) {
	root := tempRoot(t)
	path := filepath.Join(root, "data")
	limit := int64(readChunk)
	writeFile(t, path, strings.Repeat("x", 3*readChunk))
	fd, err := OpenNoFollow(path)
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = Close(fd) }()
	data, err := ReadAll(fd, path, limit)
	if err != nil || int64(len(data)) <= limit || int64(len(data)) > limit+readChunk {
		t.Fatalf("ReadAll returned %d bytes, %v; want in (%d, %d]", len(data), err, limit, limit+readChunk)
	}
}

func TestCanonicalizeResolvesLinksAndKeepsSignatures(t *testing.T) {
	root := tempRoot(t)
	directory := filepath.Join(root, "real")
	if err := os.Mkdir(directory, 0o700); err != nil {
		t.Fatal(err)
	}
	link := filepath.Join(root, "alias")
	if err := os.Symlink(directory, link); err != nil {
		t.Fatal(err)
	}
	for _, path := range []string{directory, link, link + "/../real"} {
		resolved, err := Canonicalize(path)
		if err != nil || resolved != directory {
			t.Errorf("Canonicalize(%s) = %q, %v", path, resolved, err)
		}
	}
	locked := filepath.Join(root, "locked")
	if err := os.Mkdir(locked, 0o000); err != nil {
		t.Fatal(err)
	}
	lockedLink := filepath.Join(root, "lockedlink")
	if err := os.Symlink(locked, lockedLink); err != nil {
		t.Fatal(err)
	}
	for _, path := range []string{locked, lockedLink} {
		if resolved, err := Canonicalize(path); err != nil || resolved != locked {
			t.Errorf("Canonicalize(%s) = %q, %v; want %q", path, resolved, err, locked)
		}
	}
	if _, err := Canonicalize(filepath.Join(root, "absent")); !errors.Is(err, os.ErrNotExist) {
		t.Fatalf("absent: %v", err)
	}
	want, err := Lstat(directory)
	if err != nil {
		t.Fatal(err)
	}
	resolved, err := Canonicalize(link)
	if err != nil {
		t.Fatal(err)
	}
	if got, err := Lstat(resolved); err != nil || got != want {
		t.Fatalf("Lstat(Canonicalize(link)) = %+v, %v; want %+v", got, err, want)
	}
}

func TestCanonicalizeResolvesDotDotAfterLinkPhysicallyForUnreadableTarget(t *testing.T) {
	root := tempRoot(t)
	deep := filepath.Join(root, "outer", "deep")
	if err := os.MkdirAll(deep, 0o700); err != nil {
		t.Fatal(err)
	}
	physical := filepath.Join(root, "outer", "x")
	if err := os.Mkdir(physical, 0o000); err != nil {
		t.Fatal(err)
	}
	if err := os.Mkdir(filepath.Join(root, "x"), 0o000); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(deep, filepath.Join(root, "link")); err != nil {
		t.Fatal(err)
	}
	if resolved, err := Canonicalize(root + "/link/../x"); err != nil || resolved != physical {
		t.Errorf("Canonicalize(link/../x) = %q, %v; want %q", resolved, err, physical)
	}
	t.Chdir(root)
	if resolved, err := Canonicalize("link/../x"); err != nil || resolved != physical {
		t.Errorf("Canonicalize(relative link/../x) = %q, %v; want %q", resolved, err, physical)
	}
}

func TestReadlinkGrowsItsBufferUntilTheTargetFits(t *testing.T) {
	root := tempRoot(t)
	link := filepath.Join(root, "link")
	target := strings.Repeat("a", 20)
	if err := os.Symlink(target, link); err != nil {
		t.Fatal(err)
	}
	got, err := readlinkWithin(link, 8, 64)
	if err != nil || got != target {
		t.Fatalf("readlinkWithin = %q, %v, want %q", got, err, target)
	}
	got, err = readlinkWithin(link, 20, 20)
	if !errors.Is(err, syscall.ENAMETOOLONG) || got != "" {
		t.Fatalf("readlinkWithin at the cap = %q, %v, want ENAMETOOLONG", got, err)
	}
}

func TestReadlinkReportsAPathErrorForANonLink(t *testing.T) {
	path := filepath.Join(tempRoot(t), "file")
	writeFile(t, path, "x")
	_, err := Readlink(path)
	var pathErr *fs.PathError
	if !errors.As(err, &pathErr) || pathErr.Op != "readlink" || !errors.Is(err, syscall.EINVAL) {
		t.Fatalf("Readlink = %v, want a readlink PathError with EINVAL", err)
	}
}

func TestReadDirNamesRefusesANonDirectory(t *testing.T) {
	path := filepath.Join(tempRoot(t), "file")
	writeFile(t, path, "x")
	names, err := ReadDirNames(path)
	if !errors.Is(err, syscall.ENOTDIR) || names != nil {
		t.Fatalf("ReadDirNames = %v, %v, want ENOTDIR", names, err)
	}
}

func TestWriteAllRefusesAZeroByteWrite(t *testing.T) {
	calls := 0
	stalled := func([]byte) (int, error) {
		calls++
		return 0, nil
	}
	err := writeAll(stalled, "staged", []byte("payload"))
	var pathErr *fs.PathError
	if !errors.As(err, &pathErr) || pathErr.Op != "write" || pathErr.Path != "staged" || !errors.Is(err, io.ErrShortWrite) {
		t.Fatalf("writeAll = %v, want a write PathError wrapping io.ErrShortWrite", err)
	}
	if calls != 1 {
		t.Errorf("writeAll called write %d times, want 1", calls)
	}
	var written []byte
	trickle := func(chunk []byte) (int, error) {
		written = append(written, chunk[0])
		return 1, nil
	}
	if err := writeAll(trickle, "staged", []byte("payload")); err != nil {
		t.Fatal(err)
	}
	if string(written) != "payload" {
		t.Errorf("partial writes produced %q", written)
	}
}

func TestReadDirNamesNoFollowListsBoundedAndRefusesLinks(t *testing.T) {
	root := tempRoot(t)
	directory := filepath.Join(root, "directory")
	if err := os.Mkdir(directory, 0o700); err != nil {
		t.Fatal(err)
	}
	for _, name := range []string{"a", "b", "c"} {
		writeFile(t, filepath.Join(directory, name), name)
	}
	names, opened, err := ReadDirNamesNoFollow(directory, 1)
	if err != nil || len(names) != 2 {
		t.Fatalf("ReadDirNamesNoFollow(limit 1) = %v, %v", names, err)
	}
	if want, err := LstatMetadata(directory); err != nil || opened != want {
		t.Errorf("opened metadata = %+v, want %+v (%v)", opened, want, err)
	}
	names, _, err = ReadDirNamesNoFollow(directory, 10)
	if err != nil || len(names) != 3 {
		t.Fatalf("ReadDirNamesNoFollow(limit 10) = %v, %v", names, err)
	}
	link := filepath.Join(root, "link")
	if err := os.Symlink(directory, link); err != nil {
		t.Fatal(err)
	}
	_, _, err = ReadDirNamesNoFollow(link, 10)
	if !errors.Is(err, syscall.ELOOP) && !errors.Is(err, syscall.ENOTDIR) {
		t.Fatalf("ReadDirNamesNoFollow through a link = %v, want ELOOP or ENOTDIR", err)
	}
}
