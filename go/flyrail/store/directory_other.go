//go:build !darwin && !linux

package store

import (
	"os"
	"syscall"
)

const createFlags = 0

func platformSupport(path string) error {
	return &IOError{Message: "directory state stores are unsupported on this platform", Path: path, Errno: syscall.ENOTSUP}
}

func renameReplacing(from, to string) error {
	return os.Rename(from, to)
}

func removeFile(path string) error {
	return os.Remove(path)
}
