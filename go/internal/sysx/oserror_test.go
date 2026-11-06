package sysx

import (
	"errors"
	"os"
	"runtime"
	"syscall"
	"testing"
)

func TestErrnoTextUsesLibcWording(t *testing.T) {
	if runtime.GOOS != "darwin" && runtime.GOOS != "linux" {
		t.Skip("errno tables cover darwin and linux")
	}
	cases := map[syscall.Errno]string{
		syscall.EPERM:  "Operation not permitted",
		syscall.ENOENT: "No such file or directory",
		syscall.EACCES: "Permission denied",
		syscall.EEXIST: "File exists",
		syscall.EXDEV:  map[string]string{"darwin": "Cross-device link", "linux": "Invalid cross-device link"}[runtime.GOOS],
	}
	for errno, expected := range cases {
		if got := ErrnoText(errno); got != expected {
			t.Errorf("ErrnoText(%d) = %q, want %q", errno, got, expected)
		}
	}
	unknown := map[string]string{"darwin": "Unknown error: 9999", "linux": "Unknown error 9999"}[runtime.GOOS]
	if got := ErrnoText(9999); got != unknown {
		t.Errorf("ErrnoText(9999) = %q, want %q", got, unknown)
	}
}

func TestOSErrorMessagesMatchPython(t *testing.T) {
	if runtime.GOOS != "darwin" && runtime.GOOS != "linux" {
		t.Skip("errno tables cover darwin and linux")
	}
	cases := []struct{ got, expected string }{
		{ErrnoMessage(syscall.ENOENT), "[Errno 2] No such file or directory"},
		{PathErrorMessage(syscall.ENOENT, "/tmp/a b"), "[Errno 2] No such file or directory: '/tmp/a b'"},
		{PathErrorMessage(syscall.EACCES, "/tmp/it's"), `[Errno 13] Permission denied: "/tmp/it's"`},
		{PathErrorMessage(syscall.ENOENT, "/tmp/\xffx\n"), "[Errno 2] No such file or directory: '/tmp/\ufffdx\\n'"},
		{PosixPathErrorMessage(syscall.EEXIST, "/tmp/x"), "[Errno 17] File exists: PosixPath('/tmp/x')"},
	}
	for _, c := range cases {
		if c.got != c.expected {
			t.Errorf("message = %q, want %q", c.got, c.expected)
		}
	}
}

func TestOSErrorMessagesTakeTheErrnoFromTheErrorChain(t *testing.T) {
	if runtime.GOOS != "darwin" && runtime.GOOS != "linux" {
		t.Skip("errno tables cover darwin and linux")
	}
	wrapped := &os.PathError{Op: "open", Path: "/elsewhere", Err: syscall.ENOENT}
	type outcome struct {
		message string
		errno   syscall.Errno
	}
	pair := func(message string, errno syscall.Errno) outcome { return outcome{message, errno} }
	for name, c := range map[string]struct{ got, want outcome }{
		"path":         {pair(OSErrorMessage(wrapped, "/tmp/x")), outcome{"[Errno 2] No such file or directory: '/tmp/x'", syscall.ENOENT}},
		"NUL path":     {pair(OSErrorMessage(wrapped, "/tmp/\x00")), outcome{"file name contained an unexpected NUL byte", 0}},
		"no errno":     {pair(OSErrorMessage(errors.New("plain"), "/tmp/x")), outcome{"plain", 0}},
		"without path": {pair(OSErrorMessageWithoutPath(wrapped)), outcome{"[Errno 2] No such file or directory", syscall.ENOENT}},
	} {
		if c.got != c.want {
			t.Errorf("%s: %+v, want %+v", name, c.got, c.want)
		}
	}
	texts := []struct{ got, want string }{
		{RustOSErrorText(wrapped), "No such file or directory (os error 2)"},
		{RustOSErrorText(errors.New("plain")), "plain"},
		{PathRefusalMessage(syscall.EACCES, "state store must be private to its owner", "/s"), "[Errno 13] state store must be private to its owner: '/s'"},
		{PosixPathRefusalMessage(syscall.ENOTSUP, "extended ACLs are unsupported", "/s"), "[Errno 45] extended ACLs are unsupported: PosixPath('/s')"},
	}
	if runtime.GOOS == "linux" {
		texts[3].want = "[Errno 95] extended ACLs are unsupported: PosixPath('/s')"
	}
	for _, c := range texts {
		if c.got != c.want {
			t.Errorf("message = %q, want %q", c.got, c.want)
		}
	}
}
