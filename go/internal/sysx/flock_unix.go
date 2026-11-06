//go:build darwin || linux

package sysx

import (
	"errors"
	"os"
	"sync"

	"golang.org/x/sys/unix"
)

// FileLock is an exclusive flock held on a descriptor the lock owns.
type FileLock struct {
	file *os.File
	once sync.Once
	err  error
}

// OpenLockFile opens path for locking, creating it with mode 0600 and refusing a final symbolic link.
func OpenLockFile(path string) (*FileLock, error) {
	file, err := os.OpenFile(path, os.O_RDWR|os.O_APPEND|os.O_CREATE|unix.O_NOFOLLOW|unix.O_CLOEXEC, 0o600)
	if err != nil {
		return nil, err
	}
	return &FileLock{file: file}, nil
}

// TryLock makes one non-blocking attempt at the exclusive lock; acquired is false while another descriptor holds it.
func (l *FileLock) TryLock() (acquired bool, err error) {
	raw, err := l.file.SyscallConn()
	if err != nil {
		return false, err
	}
	var lockErr error
	controlErr := raw.Control(func(fd uintptr) {
		lockErr = Retry(func() error { return unix.Flock(int(fd), unix.LOCK_EX|unix.LOCK_NB) })
	})
	if controlErr != nil {
		return false, controlErr
	}
	if errors.Is(lockErr, unix.EWOULDBLOCK) || errors.Is(lockErr, unix.EACCES) || errors.Is(lockErr, unix.EDEADLK) {
		return false, nil
	}
	if lockErr != nil {
		return false, &os.PathError{Op: "flock", Path: l.file.Name(), Err: lockErr}
	}
	return true, nil
}

// Unlock releases the lock by closing its descriptor; later calls return the first call's result.
func (l *FileLock) Unlock() error {
	l.once.Do(func() { l.err = l.file.Close() })
	return l.err
}
