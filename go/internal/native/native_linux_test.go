package native

import (
	"os"
	"path/filepath"
	"strings"
	"syscall"
	"testing"

	"golang.org/x/sys/unix"
)

const birthTimesCompared = false

func TestLinuxXattrsRefuseResourcesAndCaptureForAncestors(t *testing.T) {
	path := filepath.Join(tempRoot(t), "resource.txt")
	writeFile(t, path, "payload", 0o600)
	if err := unix.Lsetxattr(path, "user.flyrail", []byte("private"), 0); err != nil {
		t.Fatalf("setxattr user.flyrail: %v", err)
	}
	_, err := Security(path, false)
	failure := nativeError(t, err)
	want := posixPathMessage(syscall.ENOTSUP, "extended attributes and ACLs are unsupported", path)
	if failure.Message != want || failure.Errno != syscall.ENOTSUP || failure.Path != path {
		t.Fatalf("error = %+v, want %q", failure, want)
	}
	if got := readFile(t, path); got != "payload" {
		t.Fatalf("resource = %q after a refused capture", got)
	}
	value, err := attributePayload(path, "user.flyrail")
	if err != nil || string(value) != "private" {
		t.Fatalf("attributePayload = %q, %v", value, err)
	}
	ancestor, err := Security(path, true)
	if err != nil {
		t.Fatal(err)
	}
	wantAncestor := `{"acl":"","flags":0,"schema_version":1,"xattrs":[["757365722e666c797261696c","70726976617465"]]}`
	if string(ancestor) != wantAncestor {
		t.Fatalf("ancestor capture = %s, want %s", ancestor, wantAncestor)
	}
	if err := unix.Lremovexattr(path, "user.flyrail"); err != nil {
		t.Fatal(err)
	}
	clean, err := Security(path, false)
	if err != nil || len(clean) != 0 {
		t.Fatalf("clean capture = %q, %v", clean, err)
	}
}

func TestLinuxXattrsRefuseManagementStorage(t *testing.T) {
	state := filepath.Join(tempRoot(t), "state")
	makeDir(t, state, 0o700)
	if err := unix.Lsetxattr(state, "user.flyrail", []byte("x"), 0); err != nil {
		t.Fatalf("setxattr user.flyrail: %v", err)
	}
	failure := nativeError(t, EnsurePrivate(state))
	if want := posixPathMessage(syscall.ENOTSUP, "extended attributes and ACLs are unsupported", state); failure.Message != want {
		t.Fatalf("error = %+v, want %q", failure, want)
	}
}

func TestFillOutcome(t *testing.T) {
	cases := []struct {
		filled, capacity int
		errno            syscall.Errno
		want             fill
	}{
		{7, 12, 0, fillComplete},
		{12, 12, 0, fillComplete},
		{0, 12, 0, fillComplete},
		{-1, 12, syscall.ERANGE, fillRetry},
		{13, 12, 0, fillRetry},
		{-1, 12, syscall.EACCES, fillFailed},
		{-1, 12, 0, fillFailed},
	}
	for _, c := range cases {
		if got := fillOutcome(c.filled, c.capacity, c.errno); got != c.want {
			t.Errorf("fillOutcome(%d, %d, %v) = %v, want %v", c.filled, c.capacity, c.errno, got, c.want)
		}
	}
}

func TestParseNamesKeepsEmptyNamesAndAnUnterminatedTail(t *testing.T) {
	got := parseNames([]byte("user.a\x00\x00user.b\x00tail"))
	if want := []string{"user.a", "", "user.b", "tail"}; strings.Join(got, "|") != strings.Join(want, "|") || len(got) != len(want) {
		t.Fatalf("parseNames = %q, want %q", got, want)
	}
}

func TestLinuxRejectsAPathWithNULAsAnIOError(t *testing.T) {
	_, err := Security("/tmp/a\x00b", false)
	if failure := nativeError(t, err); failure.Kind != KindIO || failure.Message != "file name contained an unexpected NUL byte" {
		t.Fatalf("error = %+v", failure)
	}
}

func TestPhysicalDirectoryResolvesLinksLikeRealpath(t *testing.T) {
	root := tempRoot(t)
	real := filepath.Join(root, "real")
	makeDir(t, real, 0o700)
	link := filepath.Join(root, "link")
	if err := os.Symlink("real", link); err != nil {
		t.Fatal(err)
	}
	got, err := PhysicalDirectory(link)
	if err != nil || got != real {
		t.Fatalf("PhysicalDirectory = %q, %v; want %q", got, err, real)
	}
}

func TestNonUTF8EntriesNeverMatchAPortableKey(t *testing.T) {
	root := tempRoot(t)
	writeFile(t, filepath.Join(root, "caf\xe9"), "x", 0o600)
	got, err := CanonicalChild(root, "café")
	if err != nil || got != root+"/café" {
		t.Fatalf("CanonicalChild = %q, %v", got, err)
	}
	missing := filepath.Join(root, "dir\xff", "absent")
	_, err = Security(missing, false)
	failure := nativeError(t, err)
	if want := "[Errno 2] No such file or directory: '" + root + "/dir�/absent'"; failure.Message != want {
		t.Fatalf("error = %+v, want %q", failure, want)
	}
	entries, err := TransactionEntries(root)
	if err != nil || len(entries) != 0 {
		t.Fatalf("TransactionEntries = %q, %v", entries, err)
	}
}

func TestSharedMountAgainstTheLiveMountTable(t *testing.T) {
	root := tempRoot(t)
	table := mountTableOrSkip(t)
	if !SharedMount(table, root, filepath.Join(root, "child")) {
		t.Fatalf("a directory and its child are on different mounts:\n%s", table)
	}
}

func mountTableOrSkip(t *testing.T) []byte {
	t.Helper()
	table, _ := mountTable()
	if len(table) == 0 {
		t.Skip("no /proc/self/mountinfo")
	}
	return table
}
