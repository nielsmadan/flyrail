//go:build !darwin && !linux

package sysx

import (
	"errors"
	"io/fs"
	"syscall"
)

// Lstat reports that source snapshots are unsupported on this platform.
func Lstat(path string) (Signature, error) {
	return Signature{}, &fs.PathError{Op: "lstat", Path: path, Err: errors.ErrUnsupported}
}

// Fstat reports that source snapshots are unsupported on this platform.
func Fstat(_ int, path string) (Signature, error) {
	return Signature{}, &fs.PathError{Op: "fstat", Path: path, Err: errors.ErrUnsupported}
}

// OpenNoFollow reports that source snapshots are unsupported on this platform.
func OpenNoFollow(path string) (int, error) {
	return -1, &fs.PathError{Op: "open", Path: path, Err: errors.ErrUnsupported}
}

// ReadAll reports that source snapshots are unsupported on this platform.
func ReadAll(_ int, path string, _ int64) ([]byte, error) {
	return nil, &fs.PathError{Op: "read", Path: path, Err: errors.ErrUnsupported}
}

// Close reports that source snapshots are unsupported on this platform.
func Close(int) error {
	return errors.ErrUnsupported
}

// Canonicalize reports that source snapshots are unsupported on this platform.
func Canonicalize(path string) (string, error) {
	return "", &fs.PathError{Op: "realpath", Path: path, Err: errors.ErrUnsupported}
}

// Getwd returns the current working directory without consulting $PWD.
func Getwd() (string, error) {
	return syscall.Getwd()
}
