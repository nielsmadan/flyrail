//go:build darwin || linux

package sysx

import (
	"errors"
	"io"
	"io/fs"
	"os"

	"golang.org/x/sys/unix"
)

const readDirBatch = 256

func metadataOf(st *unix.Stat_t) Metadata {
	return Metadata{
		Dev:       DeviceNumber(st),
		Ino:       st.Ino,
		Mode:      uint32(st.Mode),
		Nlink:     uint64(st.Nlink),
		UID:       st.Uid,
		GID:       st.Gid,
		Size:      st.Size,
		MtimeSec:  st.Mtim.Sec,
		MtimeNsec: st.Mtim.Nsec,
		CtimeSec:  st.Ctim.Sec,
		CtimeNsec: st.Ctim.Nsec,
	}
}

// LstatMetadata returns the metadata of path without following a final symbolic link.
func LstatMetadata(path string) (Metadata, error) {
	st, err := RawLstat(path)
	if err != nil {
		return Metadata{}, err
	}
	return metadataOf(&st), nil
}

// StatMetadata returns the metadata of path, following symbolic links.
func StatMetadata(path string) (Metadata, error) {
	st, err := RawStat(path)
	if err != nil {
		return Metadata{}, err
	}
	return metadataOf(&st), nil
}

// FstatMetadata returns the metadata of the open descriptor fd; path only labels the error.
func FstatMetadata(fd int, path string) (Metadata, error) {
	var st unix.Stat_t
	if err := Retry(func() error { return unix.Fstat(fd, &st) }); err != nil {
		return Metadata{}, &fs.PathError{Op: "fstat", Path: path, Err: err}
	}
	return metadataOf(&st), nil
}

// TouchNow sets the access and modification times of path to the current time, following symbolic links.
func TouchNow(path string) error {
	if err := Retry(func() error { return unix.UtimesNanoAt(unix.AT_FDCWD, path, nil, 0) }); err != nil {
		return &fs.PathError{Op: "utimensat", Path: path, Err: err}
	}
	return nil
}

// Mkdir creates the directory path with mode, failing with EEXIST when anything already exists there.
func Mkdir(path string, mode uint32) error {
	if err := Retry(func() error { return unix.Mkdir(path, mode) }); err != nil {
		return &fs.PathError{Op: "mkdir", Path: path, Err: err}
	}
	return nil
}

// Rmdir removes the empty directory path.
func Rmdir(path string) error {
	if err := Retry(func() error { return unix.Rmdir(path) }); err != nil {
		return &fs.PathError{Op: "rmdir", Path: path, Err: err}
	}
	return nil
}

// Unlink removes the non-directory path without following a final symbolic link.
func Unlink(path string) error {
	if err := Retry(func() error { return unix.Unlink(path) }); err != nil {
		return &fs.PathError{Op: "unlink", Path: path, Err: err}
	}
	return nil
}

// WorkingDirectory is the descriptor that makes the At functions resolve a name like an ordinary path.
const WorkingDirectory = unix.AT_FDCWD

// MkdirAt creates the directory name relative to the open directory dirfd with mode, failing with EEXIST when anything exists there; path only labels the error.
func MkdirAt(dirfd int, name, path string, mode uint32) error {
	if err := Retry(func() error { return unix.Mkdirat(dirfd, name, mode) }); err != nil {
		return &fs.PathError{Op: "mkdir", Path: path, Err: err}
	}
	return nil
}

// OpenDirectoryAt opens the directory name relative to the open directory dirfd read-only, refusing a final symbolic link; path only labels the error.
func OpenDirectoryAt(dirfd int, name, path string) (int, error) {
	fd, err := RetryValue(func() (int, error) {
		return unix.Openat(dirfd, name, unix.O_RDONLY|unix.O_DIRECTORY|unix.O_NOFOLLOW|unix.O_CLOEXEC, 0)
	})
	if err != nil {
		return -1, &fs.PathError{Op: "open", Path: path, Err: err}
	}
	return fd, nil
}

// CreateExclusiveAt creates the file name relative to the open directory dirfd for writing with mode 0600, failing when anything exists there, including a symbolic link; path only labels the error.
func CreateExclusiveAt(dirfd int, name, path string) (int, error) {
	fd, err := RetryValue(func() (int, error) {
		return unix.Openat(dirfd, name, unix.O_WRONLY|unix.O_CREAT|unix.O_EXCL|unix.O_NOFOLLOW|unix.O_CLOEXEC, 0o600)
	})
	if err != nil {
		return -1, &fs.PathError{Op: "open", Path: path, Err: err}
	}
	return fd, nil
}

// WriteAll writes data to fd completely; path only labels the error.
func WriteAll(fd int, path string, data []byte) error {
	return writeAll(func(chunk []byte) (int, error) { return unix.Write(fd, chunk) }, path, data)
}

func writeAll(write func([]byte) (int, error), path string, data []byte) error {
	for len(data) > 0 {
		count, err := RetryValue(func() (int, error) { return write(data) })
		if err != nil {
			return &fs.PathError{Op: "write", Path: path, Err: err}
		}
		if count <= 0 {
			return &fs.PathError{Op: "write", Path: path, Err: io.ErrShortWrite}
		}
		data = data[count:]
	}
	return nil
}

// Fchown sets the owner and group of the open descriptor fd; path only labels the error.
func Fchown(fd int, path string, uid, gid uint32) error {
	if err := Retry(func() error { return unix.Fchown(fd, int(uid), int(gid)) }); err != nil {
		return &fs.PathError{Op: "fchown", Path: path, Err: err}
	}
	return nil
}

// Fchmod sets the permission bits of the open descriptor fd; path only labels the error.
func Fchmod(fd int, path string, mode uint32) error {
	if err := Retry(func() error { return unix.Fchmod(fd, mode) }); err != nil {
		return &fs.PathError{Op: "fchmod", Path: path, Err: err}
	}
	return nil
}

// ReadDirNamesNoFollow lists the directory path without following a final symbolic link or blocking on a special file, returning at most limit+1 names in the order the system lists them and the metadata of the directory it opened.
func ReadDirNamesNoFollow(path string, limit int) ([]string, Metadata, error) {
	fd, err := RetryValue(func() (int, error) {
		return unix.Openat(unix.AT_FDCWD, path, unix.O_RDONLY|unix.O_DIRECTORY|unix.O_NOFOLLOW|unix.O_NONBLOCK|unix.O_CLOEXEC, 0)
	})
	if err != nil {
		return nil, Metadata{}, &fs.PathError{Op: "open", Path: path, Err: err}
	}
	directory := os.NewFile(uintptr(fd), path)
	defer func() { _ = directory.Close() }()
	opened, err := FstatMetadata(fd, path)
	if err != nil {
		return nil, Metadata{}, err
	}
	names := []string{}
	for len(names) <= limit {
		batch, err := directory.Readdirnames(min(limit+1-len(names), readDirBatch))
		names = append(names, batch...)
		if errors.Is(err, io.EOF) {
			break
		}
		if err != nil {
			return nil, Metadata{}, err
		}
	}
	return names, opened, nil
}
