//go:build darwin || linux

package sysx

import (
	"io/fs"
	"os"

	"golang.org/x/sys/unix"
)

const maxLinkTarget = 1 << 20

// RawLstat returns the complete lstat(2) record of path.
func RawLstat(path string) (unix.Stat_t, error) {
	var st unix.Stat_t
	if err := Retry(func() error { return unix.Lstat(path, &st) }); err != nil {
		return unix.Stat_t{}, &fs.PathError{Op: "lstat", Path: path, Err: err}
	}
	return st, nil
}

// RawStat returns the complete stat(2) record of path, following symbolic links.
func RawStat(path string) (unix.Stat_t, error) {
	var st unix.Stat_t
	if err := Retry(func() error { return unix.Stat(path, &st) }); err != nil {
		return unix.Stat_t{}, &fs.PathError{Op: "stat", Path: path, Err: err}
	}
	return st, nil
}

// OpenDirectoryNoFollow opens the directory path read-only, refusing a final symbolic link.
func OpenDirectoryNoFollow(path string) (int, error) {
	fd, err := RetryValue(func() (int, error) {
		return unix.Openat(unix.AT_FDCWD, path, unix.O_RDONLY|unix.O_DIRECTORY|unix.O_NOFOLLOW|unix.O_CLOEXEC, 0)
	})
	if err != nil {
		return -1, &fs.PathError{Op: "open", Path: path, Err: err}
	}
	return fd, nil
}

// Readlink returns the target of the symbolic link path, growing its buffer until the target fits.
func Readlink(path string) (string, error) {
	return readlinkWithin(path, unix.PathMax, maxLinkTarget)
}

func readlinkWithin(path string, first, limit int) (string, error) {
	for size := first; size <= limit; size *= 2 {
		buffer := make([]byte, size)
		count, err := RetryValue(func() (int, error) { return unix.Readlink(path, buffer) })
		if err != nil {
			return "", &fs.PathError{Op: "readlink", Path: path, Err: err}
		}
		if count < size {
			return string(buffer[:count]), nil
		}
	}
	return "", &fs.PathError{Op: "readlink", Path: path, Err: unix.ENAMETOOLONG}
}

// ReadDirNames returns the entry names of the directory path in the order the system lists them, never blocking on a special file.
func ReadDirNames(path string) ([]string, error) {
	fd, err := RetryValue(func() (int, error) {
		return unix.Openat(unix.AT_FDCWD, path, unix.O_RDONLY|unix.O_DIRECTORY|unix.O_NONBLOCK|unix.O_CLOEXEC, 0)
	})
	if err != nil {
		return nil, &fs.PathError{Op: "open", Path: path, Err: err}
	}
	directory := os.NewFile(uintptr(fd), path)
	defer func() { _ = directory.Close() }()
	return directory.Readdirnames(-1)
}

// Llistxattr lists the extended attribute names of path without following a final symbolic link; an empty dest probes the size.
func Llistxattr(path string, dest []byte) (int, error) {
	return RetryValue(func() (int, error) { return unix.Llistxattr(path, dest) })
}

// Lgetxattr reads the extended attribute name of path without following a final symbolic link; an empty dest probes the size.
func Lgetxattr(path, name string, dest []byte) (int, error) {
	return RetryValue(func() (int, error) { return unix.Lgetxattr(path, name, dest) })
}
