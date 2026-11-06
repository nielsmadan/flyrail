package sysx

import (
	"os"

	"golang.org/x/sys/unix"
)

// RenameatExclusive renames from in fromDir to to in toDir, failing with EEXIST when to exists.
func RenameatExclusive(fromDir int, from string, toDir int, to string) error {
	err := Retry(func() error { return unix.Renameat2(fromDir, from, toDir, to, unix.RENAME_NOREPLACE) })
	if err != nil {
		return &os.LinkError{Op: "rename", Old: from, New: to, Err: err}
	}
	return nil
}

// RenameExclusive renames from to to, failing with EEXIST when to exists.
func RenameExclusive(from, to string) error {
	return RenameatExclusive(unix.AT_FDCWD, from, unix.AT_FDCWD, to)
}
