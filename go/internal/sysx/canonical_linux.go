package sysx

import (
	"errors"
	"io/fs"
	"strconv"

	"golang.org/x/sys/unix"
)

const maxCanonicalPath = 1 << 20

// Canonicalize returns the absolute path with every symbolic link resolved, as realpath(3).
// Without procfs it resolves the components physically.
func Canonicalize(path string) (string, error) {
	return canonicalizeVia(path, "/proc")
}

func canonicalizeVia(path, procRoot string) (string, error) {
	fd, err := RetryValue(func() (int, error) {
		return unix.Openat(unix.AT_FDCWD, path, unix.O_PATH|unix.O_CLOEXEC, 0)
	})
	if err != nil {
		return "", &fs.PathError{Op: "realpath", Path: path, Err: err}
	}
	defer func() { _ = Close(fd) }()
	link := procRoot + "/self/fd/" + strconv.Itoa(fd)
	for size := unix.PathMax; size <= maxCanonicalPath; size *= 2 {
		buffer := make([]byte, size)
		count, err := RetryValue(func() (int, error) { return unix.Readlink(link, buffer) })
		if errors.Is(err, unix.ENOENT) {
			return canonicalizeWithoutProc(path)
		}
		if err != nil {
			return "", &fs.PathError{Op: "realpath", Path: path, Err: err}
		}
		if count < size {
			return string(buffer[:count]), nil
		}
	}
	return "", &fs.PathError{Op: "realpath", Path: path, Err: unix.ENAMETOOLONG}
}

func canonicalizeWithoutProc(path string) (string, error) {
	resolved, err := resolvePhysically(path)
	if err != nil {
		return "", &fs.PathError{Op: "realpath", Path: path, Err: err}
	}
	if resolved == "" {
		return "/", nil
	}
	return resolved, nil
}
