package native

import (
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"syscall"
	"testing"

	"golang.org/x/sys/unix"

	"github.com/nielsmadan/flyrail/go/internal/sysx"
)

const (
	birthTimesCompared = true
	userImmutable      = 0x0004
	everyoneACLText    = "!#acl 1\ngroup:ABCDEFAB-CDEF-ABCD-EFAB-CDEF0000000C:everyone:12:allow:read\n"
)

func hexText(text string) string {
	const digits = "0123456789abcdef"
	out := make([]byte, 0, 2*len(text))
	for index := range len(text) {
		out = append(out, digits[text[index]>>4], digits[text[index]&0xf])
	}
	return string(out)
}

func provenanceValue(t *testing.T, path string) (string, bool) {
	t.Helper()
	size, err := unix.Lgetxattr(path, provenance, nil)
	if err != nil || size <= 0 {
		return "", false
	}
	buffer := make([]byte, size)
	filled, err := unix.Lgetxattr(path, provenance, buffer)
	if err != nil {
		t.Fatal(err)
	}
	return string(buffer[:filled]), true
}

func setXattr(t *testing.T, path, name, value string) {
	t.Helper()
	if err := unix.Lsetxattr(path, name, []byte(value), 0); err != nil {
		t.Fatalf("setxattr %s: %v", name, err)
	}
}

func removeXattr(t *testing.T, path, name string) {
	t.Helper()
	if err := unix.Lremovexattr(path, name); err != nil {
		t.Fatalf("removexattr %s: %v", name, err)
	}
}

func setFlags(t *testing.T, path string, flags int) {
	t.Helper()
	if err := unix.Chflags(path, flags); err != nil {
		t.Fatalf("chflags %#x: %v", flags, err)
	}
}

func run(t *testing.T, command ...string) {
	t.Helper()
	output, err := exec.Command(command[0], command[1:]...).CombinedOutput()
	if err != nil {
		t.Fatalf("%v: %v: %s", command, err, output)
	}
}

func resource(t *testing.T, name, content string) string {
	t.Helper()
	path := filepath.Join(tempRoot(t), name)
	writeFile(t, path, content, 0o600)
	t.Cleanup(func() { _ = unix.Chflags(path, 0) })
	return path
}

func expectRefusal(t *testing.T, err error, message, path string) {
	t.Helper()
	failure := nativeError(t, err)
	want := posixPathMessage(syscall.ENOTSUP, message, path)
	if failure.Message != want || failure.Errno != syscall.ENOTSUP || failure.Path != path {
		t.Fatalf("error = %+v, want %q", failure, want)
	}
}

func TestResourceCaptureFramesTheProvenanceValue(t *testing.T) {
	path := resource(t, "resource.txt", "payload")
	capture, err := Security(path, false)
	if err != nil {
		t.Fatal(err)
	}
	want := ""
	if value, ok := provenanceValue(t, path); ok {
		want = provenance + "\x00" + value
	}
	if string(capture) != want {
		t.Fatalf("capture = %q, want %q", capture, want)
	}
}

func TestAncestorCapturePinsTheJSONFramingOnRealFiles(t *testing.T) {
	path := resource(t, "resource.txt", "payload")
	setXattr(t, path, "flyrail.test", "value")
	capture, err := Security(path, true)
	if err != nil {
		t.Fatal(err)
	}
	want := `{"acl":"","flags":0,"schema_version":1,"xattrs":[`
	if value, ok := provenanceValue(t, path); ok {
		want += `["` + hexText(provenance) + `","` + hexText(value) + `"],`
	}
	want += `["` + hexText("flyrail.test") + `","` + hexText("value") + `"]]}`
	if string(capture) != want {
		t.Fatalf("capture = %s, want %s", capture, want)
	}
}

func TestForeignXattrsRefuseResourcesAndManagementStorage(t *testing.T) {
	path := resource(t, "resource.txt", "payload")
	setXattr(t, path, "flyrail.test", "value")
	_, err := Security(path, false)
	expectRefusal(t, err, "extended attributes are unsupported", path)
	ancestor, err := Security(path, true)
	if err != nil {
		t.Fatal(err)
	}
	if entry := `["` + hexText("flyrail.test") + `","` + hexText("value") + `"]`; !strings.Contains(string(ancestor), entry) {
		t.Fatalf("ancestor capture %s lacks %s", ancestor, entry)
	}
	state := filepath.Join(filepath.Dir(path), "state")
	makeDir(t, state, 0o700)
	setXattr(t, state, "flyrail.test", "value")
	expectRefusal(t, EnsurePrivate(state), "extended attributes are unsupported", state)
	removeXattr(t, path, "flyrail.test")
	removeXattr(t, state, "flyrail.test")
	if _, err := Security(path, false); err != nil {
		t.Fatalf("clean capture: %v", err)
	}
	if err := EnsurePrivate(state); err != nil {
		t.Fatalf("clean state: %v", err)
	}
}

func TestFileFlagsRefuseResourcesAndAreCapturedForAncestors(t *testing.T) {
	path := resource(t, "resource.txt", "payload")
	setXattr(t, path, "flyrail.test", "value")
	setFlags(t, path, userImmutable)
	_, err := Security(path, false)
	expectRefusal(t, err, "file flags cannot be preserved safely", path)
	ancestor, err := Security(path, true)
	if err != nil {
		t.Fatal(err)
	}
	if prefix := `{"acl":"","flags":4,"schema_version":1,"xattrs":[`; !strings.HasPrefix(string(ancestor), prefix) {
		t.Fatalf("ancestor capture %s does not start with %s", ancestor, prefix)
	}
	setFlags(t, path, 0)
	_, err = Security(path, false)
	expectRefusal(t, err, "extended attributes are unsupported", path)
	removeXattr(t, path, "flyrail.test")
	setFlags(t, path, userImmutable)
	_, err = Security(path, false)
	expectRefusal(t, err, "file flags cannot be preserved safely", path)
	setFlags(t, path, 0)
	if _, err := Security(path, false); err != nil {
		t.Fatalf("clean capture: %v", err)
	}
}

func TestNativeACLAndForeignXattrRefuseCaptureBeforePublication(t *testing.T) {
	path := resource(t, "AGENTS.md", "foreign")
	before, err := Security(path, false)
	if err != nil {
		t.Fatal(err)
	}
	run(t, "chmod", "+a", "everyone allow read", path)
	_, err = Security(path, false)
	expectRefusal(t, err, "extended ACLs are unsupported", path)
	if got := readFile(t, path); got != "foreign" {
		t.Fatalf("resource = %q after a refused capture", got)
	}
	ancestor, err := Security(path, true)
	if err != nil {
		t.Fatal(err)
	}
	xattrs := ""
	if value, ok := provenanceValue(t, path); ok {
		xattrs = `["` + hexText(provenance) + `","` + hexText(value) + `"]`
	}
	want := `{"acl":"` + hexText(everyoneACLText) + `","flags":0,"schema_version":1,"xattrs":[` + xattrs + `]}`
	if string(ancestor) != want {
		t.Fatalf("ancestor capture = %s, want %s", ancestor, want)
	}

	setXattr(t, path, "flyrail.test", "value")
	setFlags(t, path, userImmutable)
	_, err = Security(path, false)
	expectRefusal(t, err, "file flags cannot be preserved safely", path)
	setFlags(t, path, 0)
	_, err = Security(path, false)
	expectRefusal(t, err, "extended attributes are unsupported", path)
	removeXattr(t, path, "flyrail.test")
	_, err = Security(path, false)
	expectRefusal(t, err, "extended ACLs are unsupported", path)

	run(t, "chmod", "-N", path)
	after, err := Security(path, false)
	if err != nil {
		t.Fatal(err)
	}
	if string(after) != string(before) {
		t.Fatalf("capture after removing native metadata = %q, want %q", after, before)
	}
	if got := readFile(t, path); got != "foreign" {
		t.Fatalf("resource = %q", got)
	}
}

func TestDarwinRejectsAPathWithNUL(t *testing.T) {
	_, err := Security("/tmp/a\x00b", false)
	if failure := nativeError(t, err); failure.Kind != KindInvalid || failure.Message != "embedded null byte" {
		t.Fatalf("error = %+v", failure)
	}
}

func TestAttributeNamesAreSortedAndUnique(t *testing.T) {
	cases := []struct {
		buffer string
		want   []string
	}{
		{"b\x00a\x00b\x00", []string{"a", "b"}},
		{"\x00\x00", []string{""}},
		{"a\x00\x00b\x00", []string{"", "a", "b"}},
		{"tail", []string{"tail"}},
	}
	for _, c := range cases {
		got := attributeNames([]byte(c.buffer))
		if strings.Join(got, "|") != strings.Join(c.want, "|") || len(got) != len(c.want) {
			t.Errorf("attributeNames(%q) = %q, want %q", c.buffer, got, c.want)
		}
	}
}

func TestPhysicalDirectoryReportsTheKernelSpellingAndRefusesAFinalLink(t *testing.T) {
	root := tempRoot(t)
	real := filepath.Join(root, "Real")
	makeDir(t, real, 0o700)
	link := filepath.Join(root, "link")
	if err := os.Symlink("Real", link); err != nil {
		t.Fatal(err)
	}
	got, err := PhysicalDirectory(real)
	if err != nil || got != real {
		t.Fatalf("PhysicalDirectory = %q, %v; want %q", got, err, real)
	}
	_, err = PhysicalDirectory(link)
	if failure := nativeError(t, err); failure.Errno != syscall.ENOTDIR || failure.Path != link {
		t.Fatalf("final link: %+v", failure)
	}
	if !caseInsensitive(t, root) {
		return
	}
	got, err = PhysicalDirectory(filepath.Join(root, "real"))
	if err != nil || got != real {
		t.Fatalf("case alias PhysicalDirectory = %q, %v; want %q", got, err, real)
	}
}

func TestLossyPathsInRefusals(t *testing.T) {
	missing := filepath.Join(tempRoot(t), "caf\xe9")
	_, err := Security(missing, false)
	failure := nativeError(t, err)
	if want := "[Errno 2] No such file or directory: '" + filepath.Dir(missing) + "/caf�'"; failure.Message != want || failure.Path != missing {
		t.Fatalf("error = %+v, want %q", failure, want)
	}
}

func TestSameObjectComparesDeviceAndInode(t *testing.T) {
	base := sysx.Signature{Dev: 7, Ino: 9}
	cases := []struct {
		name  string
		other sysx.Signature
		want  bool
	}{
		{"identical", sysx.Signature{Dev: 7, Ino: 9}, true},
		{"other inode", sysx.Signature{Dev: 7, Ino: 10}, false},
		{"other device", sysx.Signature{Dev: 8, Ino: 9}, false},
	}
	for _, c := range cases {
		if got := sameObject(base, c.other); got != c.want {
			t.Errorf("%s: sameObject = %v, want %v", c.name, got, c.want)
		}
	}
}
