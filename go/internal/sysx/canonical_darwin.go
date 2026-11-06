package sysx

import (
	"errors"
	"io/fs"
	"strings"

	"golang.org/x/sys/unix"

	"github.com/nielsmadan/flyrail/go/internal/darwin"
)

// Canonicalize returns the absolute path with every symbolic link resolved, as realpath(3).
// An object that cannot be opened for events (mode 000) is resolved through its canonical parent and keeps its final name as given.
func Canonicalize(path string) (string, error) {
	fd, err := RetryValue(func() (int, error) {
		return unix.Openat(unix.AT_FDCWD, path, unix.O_EVTONLY|unix.O_NONBLOCK|unix.O_CLOEXEC, 0)
	})
	if errors.Is(err, unix.EACCES) {
		return canonicalizeUnreadable(path)
	}
	if err != nil {
		return "", &fs.PathError{Op: "realpath", Path: path, Err: err}
	}
	defer func() { _ = Close(fd) }()
	resolved, err := darwin.GetPath(fd)
	if err != nil {
		return "", &fs.PathError{Op: "realpath", Path: path, Err: err}
	}
	return resolved, nil
}

func canonicalizeUnreadable(path string) (string, error) {
	linkFree, err := resolvePhysically(path)
	if err != nil {
		return "", &fs.PathError{Op: "realpath", Path: path, Err: err}
	}
	if linkFree == "" {
		return "/", nil
	}
	cut := strings.LastIndexByte(linkFree, '/')
	parent := linkFree[:cut]
	if parent == "" {
		parent = "/"
	}
	canonicalParent, err := Canonicalize(parent)
	if err != nil {
		return "", &fs.PathError{Op: "realpath", Path: path, Err: errors.Unwrap(err)}
	}
	return strings.TrimSuffix(canonicalParent, "/") + linkFree[cut:], nil
}
