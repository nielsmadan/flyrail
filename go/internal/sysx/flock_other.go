//go:build !darwin && !linux

package sysx

import (
	"errors"
	"os"
)

// FileLock is unavailable on this platform.
type FileLock struct{}

// OpenLockFile reports that file locks are unsupported on this platform.
func OpenLockFile(path string) (*FileLock, error) {
	return nil, &os.PathError{Op: "open", Path: path, Err: errors.ErrUnsupported}
}

// TryLock reports that file locks are unsupported on this platform.
func (*FileLock) TryLock() (bool, error) {
	return false, errors.ErrUnsupported
}

// Unlock does nothing on this platform.
func (*FileLock) Unlock() error {
	return nil
}
