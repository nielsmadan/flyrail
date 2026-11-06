package darwin

import (
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"syscall"
	"testing"
)

const everyoneACLText = "!#acl 1\ngroup:ABCDEFAB-CDEF-ABCD-EFAB-CDEF0000000C:everyone:12:allow:read\n"

func tempRoot(t *testing.T) string {
	t.Helper()
	root, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	return root
}

func closeOrFail(t *testing.T, file *os.File) {
	t.Helper()
	if err := file.Close(); err != nil {
		t.Error(err)
	}
}

func fileWithEveryoneACL(t *testing.T) string {
	t.Helper()
	path := filepath.Join(tempRoot(t), "AGENTS.md")
	if err := os.WriteFile(path, []byte("foreign"), 0o600); err != nil {
		t.Fatal(err)
	}
	output, err := exec.Command("chmod", "+a", "everyone allow read", path).CombinedOutput()
	if err != nil {
		t.Fatalf("chmod +a: %v: %s", err, output)
	}
	return path
}

func extendedText(t *testing.T, path string) string {
	t.Helper()
	acl, err := ACLGetFile(path, ACLTypeExtended)
	if err != nil {
		t.Fatalf("ACLGetFile: %v", err)
	}
	defer func() {
		if err := acl.Free(); err != nil {
			t.Errorf("Free: %v", err)
		}
	}()
	if !acl.HasEntry(ACLFirstEntry) {
		t.Fatal("ACL has no first entry")
	}
	text, err := acl.Text()
	if err != nil {
		t.Fatalf("Text: %v", err)
	}
	return string(text)
}

func TestACLFreeIsIdempotentAndGuardsLaterUse(t *testing.T) {
	acl, err := ACLGetFile(fileWithEveryoneACL(t), ACLTypeExtended)
	if err != nil {
		t.Fatalf("ACLGetFile: %v", err)
	}
	for range 2 {
		if err := acl.Free(); err != nil {
			t.Fatalf("Free: %v", err)
		}
	}
	if acl.HasEntry(ACLFirstEntry) {
		t.Fatal("HasEntry after Free = true, want false")
	}
	if _, err := acl.Text(); !errors.Is(err, syscall.EINVAL) {
		t.Fatalf("Text after Free error = %v, want EINVAL", err)
	}
}

func TestACLTextMatchesRustCapture(t *testing.T) {
	path := fileWithEveryoneACL(t)
	if got := extendedText(t, path); got != everyoneACLText {
		t.Fatalf("ACL text = %q, want %q", got, everyoneACLText)
	}
}

func TestACLTextSurvivesGarbageCollection(t *testing.T) {
	path := fileWithEveryoneACL(t)
	done := make(chan struct{})
	collected := make(chan struct{})
	go func() {
		defer close(collected)
		for {
			select {
			case <-done:
				return
			default:
				runtime.GC()
			}
		}
	}()
	defer func() {
		close(done)
		<-collected
	}()
	var retained [][]byte
	for i := range 200 {
		retained = append(retained, make([]byte, 64<<10))
		runtime.GC()
		if got := extendedText(t, path); got != everyoneACLText {
			t.Fatalf("iteration %d: ACL text = %q, want %q", i, got, everyoneACLText)
		}
		if i%16 == 0 {
			retained = nil
		}
	}
	runtime.KeepAlive(retained)
}

func TestACLGetFileReportsENOENTWithoutExtendedACL(t *testing.T) {
	path := filepath.Join(tempRoot(t), "plain")
	if err := os.WriteFile(path, []byte("plain"), 0o600); err != nil {
		t.Fatal(err)
	}
	_, err := ACLGetFile(path, ACLTypeExtended)
	if !errors.Is(err, syscall.ENOENT) {
		t.Fatalf("ACLGetFile error = %v, want ENOENT", err)
	}
}

func TestACLGetFileReportsENOENTForMissingPath(t *testing.T) {
	for range 50 {
		runtime.GC()
		_, err := ACLGetFile(filepath.Join(tempRoot(t), "missing"), ACLTypeExtended)
		if !errors.Is(err, syscall.ENOENT) {
			t.Fatalf("ACLGetFile error = %v, want ENOENT", err)
		}
	}
}

func TestACLGetFileRejectsInteriorNUL(t *testing.T) {
	_, err := ACLGetFile("bad\x00path", ACLTypeExtended)
	if !errors.Is(err, syscall.EINVAL) {
		t.Fatalf("ACLGetFile error = %v, want EINVAL", err)
	}
}

func TestGetPathReturnsOpenedPath(t *testing.T) {
	path := filepath.Join(tempRoot(t), "opened")
	file, err := os.Create(path)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { closeOrFail(t, file) })
	runtime.GC()
	got, err := GetPath(int(file.Fd()))
	if err != nil {
		t.Fatalf("GetPath: %v", err)
	}
	if got != path {
		t.Fatalf("GetPath = %q, want %q", got, path)
	}
}

func TestGetPathReportsEBADF(t *testing.T) {
	_, err := GetPath(-1)
	if !errors.Is(err, syscall.EBADF) {
		t.Fatalf("GetPath error = %v, want EBADF", err)
	}
}
