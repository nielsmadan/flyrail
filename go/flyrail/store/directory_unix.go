//go:build darwin || linux

package store

import (
	"golang.org/x/sys/unix"

	"github.com/nielsmadan/flyrail/go/internal/sysx"
)

const createFlags = unix.O_NOFOLLOW | unix.O_CLOEXEC

func platformSupport(string) error {
	return nil
}

func renameReplacing(from, to string) error {
	return sysx.Retry(func() error { return unix.Rename(from, to) })
}

func removeFile(path string) error {
	return sysx.Retry(func() error { return unix.Unlink(path) })
}
