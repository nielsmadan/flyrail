//go:build !darwin && !linux

package sysx

import (
	"errors"
	"io/fs"
)

func unsupported(op, path string) error {
	return &fs.PathError{Op: op, Path: path, Err: errors.ErrUnsupported}
}

// LstatMetadata reports that resource I/O is unsupported on this platform.
func LstatMetadata(path string) (Metadata, error) {
	return Metadata{}, unsupported("lstat", path)
}

// StatMetadata reports that resource I/O is unsupported on this platform.
func StatMetadata(path string) (Metadata, error) {
	return Metadata{}, unsupported("stat", path)
}

// FstatMetadata reports that resource I/O is unsupported on this platform.
func FstatMetadata(_ int, path string) (Metadata, error) {
	return Metadata{}, unsupported("fstat", path)
}

// TouchNow reports that resource I/O is unsupported on this platform.
func TouchNow(path string) error {
	return unsupported("utimensat", path)
}

// Mkdir reports that resource I/O is unsupported on this platform.
func Mkdir(path string, _ uint32) error {
	return unsupported("mkdir", path)
}

// Rmdir reports that resource I/O is unsupported on this platform.
func Rmdir(path string) error {
	return unsupported("rmdir", path)
}

// Unlink reports that resource I/O is unsupported on this platform.
func Unlink(path string) error {
	return unsupported("unlink", path)
}

// WorkingDirectory is the descriptor that makes the At functions resolve a name like an ordinary path.
const WorkingDirectory = -100

// MkdirAt reports that resource I/O is unsupported on this platform.
func MkdirAt(_ int, _, path string, _ uint32) error {
	return unsupported("mkdir", path)
}

// OpenDirectoryAt reports that resource I/O is unsupported on this platform.
func OpenDirectoryAt(_ int, _, path string) (int, error) {
	return -1, unsupported("open", path)
}

// CreateExclusiveAt reports that resource I/O is unsupported on this platform.
func CreateExclusiveAt(_ int, _, path string) (int, error) {
	return -1, unsupported("open", path)
}

// WriteAll reports that resource I/O is unsupported on this platform.
func WriteAll(_ int, path string, _ []byte) error {
	return unsupported("write", path)
}

// Fchown reports that resource I/O is unsupported on this platform.
func Fchown(_ int, path string, _, _ uint32) error {
	return unsupported("fchown", path)
}

// Fchmod reports that resource I/O is unsupported on this platform.
func Fchmod(_ int, path string, _ uint32) error {
	return unsupported("fchmod", path)
}

// OpenDirectoryNoFollow reports that resource I/O is unsupported on this platform.
func OpenDirectoryNoFollow(path string) (int, error) {
	return -1, unsupported("open", path)
}

// ReadDirNames reports that resource I/O is unsupported on this platform.
func ReadDirNames(path string) ([]string, error) {
	return nil, unsupported("open", path)
}

// ReadDirNamesNoFollow reports that resource I/O is unsupported on this platform.
func ReadDirNamesNoFollow(path string, _ int) ([]string, Metadata, error) {
	return nil, Metadata{}, unsupported("open", path)
}
