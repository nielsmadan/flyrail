//go:build darwin || linux

package native

import (
	"errors"
	"os"
	"path/filepath"
	"strconv"
	"syscall"
	"testing"
	"time"

	"github.com/nielsmadan/flyrail/go/internal/jsonx"
)

func tempRoot(t *testing.T) string {
	t.Helper()
	root, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	return root
}

func writeFile(t *testing.T, path, content string, mode os.FileMode) {
	t.Helper()
	if err := os.WriteFile(path, []byte(content), mode); err != nil {
		t.Fatal(err)
	}
	if err := os.Chmod(path, mode); err != nil {
		t.Fatal(err)
	}
}

func makeDir(t *testing.T, path string, mode os.FileMode) {
	t.Helper()
	if err := os.Mkdir(path, mode); err != nil {
		t.Fatal(err)
	}
	if err := os.Chmod(path, mode); err != nil {
		t.Fatal(err)
	}
}

func readFile(t *testing.T, path string) string {
	t.Helper()
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	return string(data)
}

func nativeError(t *testing.T, err error) *Error {
	t.Helper()
	var failure *Error
	if !errors.As(err, &failure) {
		t.Fatalf("error %v (%T) is not a *native.Error", err, err)
	}
	return failure
}

func posixPathMessage(errno syscall.Errno, message, path string) string {
	return "[Errno " + strconv.Itoa(int(errno)) + "] " + message + ": PosixPath(" + jsonx.PythonStrRepr(path) + ")"
}

func expectUnsafe(t *testing.T, err error, message, path string) {
	t.Helper()
	failure := nativeError(t, err)
	if failure.Kind != KindUnsafe || failure.Message != message || failure.Path != path {
		t.Fatalf("error = %+v, want unsafe %q at %q", failure, message, path)
	}
}

func caseInsensitive(t *testing.T, root string) bool {
	t.Helper()
	probe := filepath.Join(root, "CaseProbe")
	makeDir(t, probe, 0o700)
	_, err := os.Lstat(filepath.Join(root, "caseprobe"))
	if err := os.Remove(probe); err != nil {
		t.Fatal(err)
	}
	return err == nil
}

func TestCaptureOfAMissingPathReportsThePythonOSError(t *testing.T) {
	missing := filepath.Join(tempRoot(t), "missing.txt")
	for _, ancestor := range []bool{false, true} {
		_, err := Security(missing, ancestor)
		failure := nativeError(t, err)
		want := "[Errno 2] No such file or directory: " + jsonx.PythonStrRepr(missing)
		if failure.Kind != KindIO || failure.Errno != syscall.ENOENT || failure.Message != want || failure.Path != missing {
			t.Fatalf("ancestor %v: error = %+v, want %q", ancestor, failure, want)
		}
		if !errors.Is(err, syscall.ENOENT) {
			t.Fatalf("ancestor %v: error does not wrap ENOENT", ancestor)
		}
	}
}

func TestCaptureOfAPlainFileFramesOnlyAllowedMetadata(t *testing.T) {
	root := tempRoot(t)
	path := filepath.Join(root, "plain")
	makeDir(t, path, 0o700)
	ancestor, err := Security(path, true)
	if err != nil {
		t.Fatal(err)
	}
	prefix := `{"acl":"","flags":0,"schema_version":1,"xattrs":[`
	if got := string(ancestor); len(got) < len(prefix) || got[:len(prefix)] != prefix || got[len(got)-2:] != "]}" {
		t.Fatalf("ancestor capture = %s", got)
	}
}

func TestEnsurePrivateRefusesForeignGroupOrOtherAccess(t *testing.T) {
	root := tempRoot(t)
	for _, mode := range []os.FileMode{0o750, 0o755, 0o070, 0o007} {
		state := filepath.Join(root, "state-"+strconv.FormatUint(uint64(mode), 8))
		makeDir(t, state, mode)
		failure := nativeError(t, EnsurePrivate(state))
		want := posixPathMessage(syscall.ENOTSUP, "management storage must be private to its owner", state)
		if failure.Kind != KindIO || failure.Errno != syscall.ENOTSUP || failure.Message != want || failure.Path != state {
			t.Errorf("mode %o: error = %+v, want %q", mode, failure, want)
		}
		if err := os.Chmod(state, 0o700); err != nil {
			t.Fatal(err)
		}
	}
	state := filepath.Join(root, "state-private")
	makeDir(t, state, 0o700)
	if err := EnsurePrivate(state); err != nil {
		t.Fatalf("owner-only directory: %v", err)
	}
}

func TestEnsurePrivateChecksThePayloadFileNotOnlyItsParent(t *testing.T) {
	parentDir := filepath.Join(tempRoot(t), "state")
	makeDir(t, parentDir, 0o700)
	backup := filepath.Join(parentDir, "backup")
	writeFile(t, backup, "backup", 0o640)
	failure := nativeError(t, EnsurePrivate(backup))
	if failure.Kind != KindIO || failure.Errno != syscall.ENOTSUP || failure.Path != backup {
		t.Fatalf("group-readable payload: error = %+v", failure)
	}
	if err := os.Chmod(backup, 0o600); err != nil {
		t.Fatal(err)
	}
	if err := EnsurePrivate(backup); err != nil {
		t.Fatalf("owner-only payload: %v", err)
	}
}

func TestEnsurePrivateReportsAMissingPathByErrno(t *testing.T) {
	missing := filepath.Join(tempRoot(t), "absent")
	failure := nativeError(t, EnsurePrivate(missing))
	if failure.Errno != syscall.ENOENT || failure.Message != "[Errno 2] No such file or directory: "+jsonx.PythonStrRepr(missing) {
		t.Fatalf("error = %+v", failure)
	}
}

func TestCanonicalDestinationKeepsAnAbsentTailUnderThePhysicalParent(t *testing.T) {
	root := tempRoot(t)
	makeDir(t, filepath.Join(root, "Config"), 0o700)
	got, err := CanonicalDestination(root + "//Config/./absent/deeper/config.json")
	if err != nil {
		t.Fatal(err)
	}
	if want := root + "/Config/absent/deeper/config.json"; got != want {
		t.Fatalf("CanonicalDestination = %q, want %q", got, want)
	}
}

func TestCanonicalDestinationRejectsRootsAndTraversal(t *testing.T) {
	cases := map[string]string{
		"":          "resource destination must be a nonempty string path without NUL",
		"/a\x00b":   "resource destination must be a nonempty string path without NUL",
		"/":         "resource destination cannot be a root or contain traversal",
		"/a/../b":   "resource destination cannot be a root or contain traversal",
		"relative":  "resource destination cannot be a root or contain traversal",
		"/tmp/\xff": "resource destination must be a nonempty string path without NUL",
	}
	for path, want := range cases {
		_, err := CanonicalDestination(path)
		failure := nativeError(t, err)
		if failure.Kind != KindInvalid || failure.Message != want {
			t.Errorf("CanonicalDestination(%q) = %+v, want %q", path, failure, want)
		}
	}
}

func TestCanonicalDestinationIOFailuresSurfacePythonOSErrorText(t *testing.T) {
	if os.Geteuid() == 0 {
		t.Skip("root ignores directory permissions")
	}
	locked := filepath.Join(tempRoot(t), "locked")
	makeDir(t, locked, 0o000)
	t.Cleanup(func() { _ = os.Chmod(locked, 0o700) })
	_, err := CanonicalDestination(locked + "/child.md")
	failure := nativeError(t, err)
	want := "[Errno 13] Permission denied: " + jsonx.PythonStrRepr(locked)
	if failure.Message != want || failure.Path != locked || failure.Errno != syscall.EACCES {
		t.Fatalf("error = %+v, want %q", failure, want)
	}
}

func TestCaseAliasesShareAuthorityOnlyOnCaseInsensitiveHosts(t *testing.T) {
	root := tempRoot(t)
	original := filepath.Join(root, "AGENTS.md")
	writeFile(t, original, "text", 0o600)
	alias := filepath.Join(root, "agents.md")
	if caseInsensitive(t, root) {
		got, err := CanonicalDestination(alias)
		if err != nil {
			t.Fatal(err)
		}
		if got != original {
			t.Fatalf("alias canonicalizes to %q, want %q", got, original)
		}
		return
	}
	_, err := CanonicalDestination(alias)
	expectUnsafe(t, err, "resource path has a conflicting portable spelling", alias)
	writeFile(t, alias, "different file", 0o600)
	_, err = CanonicalDestination(original)
	expectUnsafe(t, err, "portable resource path collision", original)
}

func TestAncestorCaseAliasesResolveToTheStoredSpelling(t *testing.T) {
	root := tempRoot(t)
	makeDir(t, filepath.Join(root, "Config"), 0o700)
	if !caseInsensitive(t, root) {
		_, err := CanonicalDestination(root + "/config/absent/config.json")
		expectUnsafe(t, err, "resource path has a conflicting portable spelling", root+"/config")
		return
	}
	first, err := CanonicalDestination(root + "/Config/absent/config.json")
	if err != nil {
		t.Fatal(err)
	}
	second, err := CanonicalDestination(root + "/config/absent/config.json")
	if err != nil {
		t.Fatal(err)
	}
	if first != second || first != root+"/Config/absent/config.json" {
		t.Fatalf("aliases canonicalize to %q and %q", first, second)
	}
}

func TestHardLinkedDocumentsAreAmbiguous(t *testing.T) {
	root := tempRoot(t)
	first, second := filepath.Join(root, "first.md"), filepath.Join(root, "second.md")
	writeFile(t, first, "same inode", 0o600)
	if err := os.Link(first, second); err != nil {
		t.Fatal(err)
	}
	for _, path := range []string{first, second} {
		_, err := CanonicalDestination(path)
		expectUnsafe(t, err, "hard-linked resources are ambiguous: "+path, path)
	}
}

func TestLinksAreRefusedOnTheWayToADestination(t *testing.T) {
	root := tempRoot(t)
	tree := filepath.Join(root, "tree")
	external := filepath.Join(root, "external")
	makeDir(t, tree, 0o700)
	makeDir(t, external, 0o700)
	link := filepath.Join(tree, "link")
	if err := os.Symlink(external, link); err != nil {
		t.Fatal(err)
	}
	_, err := CanonicalDestination(link + "/file")
	expectUnsafe(t, err, "resource links and special files are unsupported: "+link, link)
	_, err = CanonicalDestination(link)
	expectUnsafe(t, err, "resource links and special files are unsupported: "+link, link)
	_, err = TransactionEntries(tree)
	expectUnsafe(t, err, "resource links and special files are unsupported: "+link, link)
}

func TestARegularFileAncestorIsRefused(t *testing.T) {
	root := tempRoot(t)
	file := filepath.Join(root, "file")
	writeFile(t, file, "x", 0o600)
	_, err := CanonicalDestination(file + "/child")
	expectUnsafe(t, err, "resource ancestor must be a directory", file)
}

func TestCanonicalChildKeepsAManagementSpelling(t *testing.T) {
	root := tempRoot(t)
	physical := filepath.Join(root, ".FLYRAIL-deadbeef.TXN")
	makeDir(t, physical, 0o700)
	direct, err := CanonicalChild(physical, "receipt.json")
	if err != nil {
		t.Fatal(err)
	}
	if direct != physical+"/receipt.json" {
		t.Fatalf("CanonicalChild = %q", direct)
	}
	if !ManagementName(filepath.Base(filepath.Dir(direct))) {
		t.Fatalf("parent of %q is not a management name", direct)
	}
}

func TestPhysicalLocationResolvesLinksAndKeepsTheMissingTail(t *testing.T) {
	root := tempRoot(t)
	real := filepath.Join(root, "real")
	makeDir(t, real, 0o700)
	if err := os.Symlink("real", filepath.Join(root, "alias")); err != nil {
		t.Fatal(err)
	}
	got, err := PhysicalLocation(root + "/alias/./missing/../absent/leaf")
	if err != nil {
		t.Fatal(err)
	}
	if want := real + "/absent/leaf"; got != want {
		t.Fatalf("PhysicalLocation = %q, want %q", got, want)
	}
	t.Chdir(real)
	relative, err := PhysicalLocation("staging/x")
	if err != nil {
		t.Fatal(err)
	}
	if want := real + "/staging/x"; relative != want {
		t.Fatalf("relative PhysicalLocation = %q, want %q", relative, want)
	}
	failure := nativeError(t, func() error { _, err := PhysicalLocation(""); return err }())
	if failure.Message != "cannot make an empty path absolute" {
		t.Fatalf("empty path: %+v", failure)
	}
}

func TestTransactionEntriesFindsCaseAliasedFences(t *testing.T) {
	root := tempRoot(t)
	tree := filepath.Join(root, "tree")
	fence := filepath.Join(tree, "nested", ".FLYRAIL-deadbeef.TXN")
	if err := os.MkdirAll(fence, 0o700); err != nil {
		t.Fatal(err)
	}
	writeFile(t, filepath.Join(tree, "nested", "file.md"), "x", 0o600)
	entries, err := TransactionEntries(tree)
	if err != nil {
		t.Fatal(err)
	}
	if len(entries) != 1 || entries[0] != fence {
		t.Fatalf("TransactionEntries = %q, want [%q]", entries, fence)
	}
	file := filepath.Join(tree, "nested", "file.md")
	_, err = TransactionEntries(file)
	expectUnsafe(t, err, "whole-tree resource must be a directory", file)
}

func TestManagedSpellingRefusesAmbiguousStateRoots(t *testing.T) {
	root := tempRoot(t)
	state := filepath.Join(root, "State")
	makeDir(t, state, 0o700)
	if err := ManagedSpelling(state); err != nil {
		t.Fatalf("exact spelling: %v", err)
	}
	if err := ManagedSpelling(filepath.Join(root, "absent", "state")); err != nil {
		t.Fatalf("absent parent: %v", err)
	}
	lower := filepath.Join(root, "state")
	if caseInsensitive(t, root) {
		expectUnsafe(t, ManagedSpelling(lower), "managed directory spelling mismatch", lower)
	} else {
		makeDir(t, lower, 0o700)
		expectUnsafe(t, ManagedSpelling(state), "portable filename collision", state)
	}
	file := filepath.Join(root, "file")
	writeFile(t, file, "x", 0o600)
	expectUnsafe(t, ManagedSpelling(file), "expected a directory", file)
	expectUnsafe(t, ManagedSpelling(filepath.Join(file, "child")), "resource ancestor is not an ordinary directory", file)
	link := filepath.Join(root, "link")
	if err := os.Symlink(state, link); err != nil {
		t.Fatal(err)
	}
	expectUnsafe(t, ManagedSpelling(link), "managed symlinks, reparse points and special files are unsupported", link)
}

func TestSameMountWithinOneDirectory(t *testing.T) {
	root := tempRoot(t)
	makeDir(t, filepath.Join(root, "a"), 0o700)
	same, err := SameMount(filepath.Join(root, "a"), filepath.Join(root, "missing", "b"))
	if err != nil {
		t.Fatal(err)
	}
	if !same {
		t.Fatal("two paths in one temporary directory are on different mounts")
	}
}

func TestADestinationModifiedBeforeTheBoundaryIsUnchangedWheneverItWasBorn(t *testing.T) {
	path := filepath.Join(tempRoot(t), "AGENTS.md")
	writeFile(t, path, "# mine\n", 0o600)
	earlier := time.Now().Add(-2 * time.Minute).Truncate(time.Second)
	if err := os.Chtimes(path, earlier, earlier); err != nil {
		t.Fatal(err)
	}
	times, err := FileTimes(path)
	if err != nil {
		t.Fatal(err)
	}
	if !times.Modified.Equal(earlier) {
		t.Fatalf("Modified = %v, want %v", times.Modified, earlier)
	}
	if times.HasBorn != birthTimesCompared {
		t.Fatalf("HasBorn = %v, want %v", times.HasBorn, birthTimesCompared)
	}
	if times.ChangedAfter(earlier.Add(time.Minute)) {
		t.Fatalf("a file modified before the boundary changed after it: %+v", times)
	}
	if !times.ChangedAfter(earlier.Add(-time.Second)) {
		t.Fatalf("a file modified after the boundary is unchanged: %+v", times)
	}
	_, err = FileTimes(filepath.Join(filepath.Dir(path), "absent"))
	if failure := nativeError(t, err); failure.Errno != syscall.ENOENT {
		t.Fatalf("missing path: %+v", failure)
	}
}

func TestCanonicalChildRefusesAParentThatIsNotADirectory(t *testing.T) {
	root := tempRoot(t)
	fifo := filepath.Join(root, "fifo")
	if err := syscall.Mkfifo(fifo, 0o600); err != nil {
		t.Fatal(err)
	}
	_, err := CanonicalChild(fifo, "child")
	if failure := nativeError(t, err); failure.Errno != syscall.ENOTDIR || failure.Path != fifo {
		t.Fatalf("error = %+v", failure)
	}
}

func TestSameMountIsFalseAcrossDevices(t *testing.T) {
	root := tempRoot(t)
	other := "/dev"
	if _, err := os.Stat(other); err != nil {
		t.Skip("no /dev")
	}
	first, err := device(root)
	if err != nil {
		t.Fatal(err)
	}
	second, err := device(other)
	if err != nil {
		t.Fatal(err)
	}
	if first == second {
		t.Skip("/dev shares a device with the temporary directory")
	}
	same, err := SameMount(root, other)
	if err != nil || same {
		t.Fatalf("SameMount(%q, %q) = %v, %v; want false", root, other, same, err)
	}
}

func TestSameMountReportsAMissingRoot(t *testing.T) {
	_, err := SameMount("relative-does-not-exist-anywhere/x", tempRoot(t))
	if err == nil {
		t.Fatal("SameMount accepted a path with no existing ancestor")
	}
}
