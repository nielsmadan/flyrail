//go:build darwin || linux

package sysx

import (
	"io/fs"

	"golang.org/x/sys/unix"
)

const readChunk = 64 * 1024

func signatureOf(st *unix.Stat_t) Signature {
	return Signature{
		Dev:   DeviceNumber(st),
		Ino:   st.Ino,
		Mode:  uint32(st.Mode),
		Size:  st.Size,
		Mtime: st.Mtim.Nano(),
		Ctime: st.Ctim.Nano(),
	}
}

// Lstat returns the signature of path without following a final symbolic link.
func Lstat(path string) (Signature, error) {
	var st unix.Stat_t
	if err := Retry(func() error { return unix.Lstat(path, &st) }); err != nil {
		return Signature{}, &fs.PathError{Op: "lstat", Path: path, Err: err}
	}
	return signatureOf(&st), nil
}

// Fstat returns the signature of the open descriptor fd; path only labels the error.
func Fstat(fd int, path string) (Signature, error) {
	var st unix.Stat_t
	if err := Retry(func() error { return unix.Fstat(fd, &st) }); err != nil {
		return Signature{}, &fs.PathError{Op: "fstat", Path: path, Err: err}
	}
	return signatureOf(&st), nil
}

// OpenNoFollow opens path read-only without following a final symbolic link and without blocking on special files.
func OpenNoFollow(path string) (int, error) {
	fd, err := RetryValue(func() (int, error) {
		return unix.Openat(unix.AT_FDCWD, path, unix.O_RDONLY|unix.O_NOFOLLOW|unix.O_NONBLOCK|unix.O_CLOEXEC, 0)
	})
	if err != nil {
		return -1, &fs.PathError{Op: "open", Path: path, Err: err}
	}
	return fd, nil
}

// ReadAll reads fd to its end; with limit >= 0 it stops after limit+1 bytes so callers can detect an oversized file.
func ReadAll(fd int, path string, limit int64) ([]byte, error) {
	var data []byte
	buffer := make([]byte, readChunk)
	for limit < 0 || int64(len(data)) <= limit {
		count, err := RetryValue(func() (int, error) { return unix.Read(fd, buffer) })
		if err != nil {
			return nil, &fs.PathError{Op: "read", Path: path, Err: err}
		}
		if count == 0 {
			break
		}
		data = append(data, buffer[:count]...)
	}
	return data, nil
}

// Close closes fd once; EINTR is not retried because the descriptor is already released.
func Close(fd int) error {
	return unix.Close(fd)
}

// Getwd returns the current working directory from getcwd(3), without consulting $PWD.
func Getwd() (string, error) {
	return RetryValue(unix.Getwd)
}
