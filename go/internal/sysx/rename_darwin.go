package sysx

import (
	"os"

	"golang.org/x/sys/unix"
)

// RenameatExclusive renames from in fromDir to to in toDir, failing with EEXIST when to exists.
func RenameatExclusive(fromDir int, from string, toDir int, to string) error {
	err := Retry(func() error { return unix.RenameatxNp(fromDir, from, toDir, to, unix.RENAME_EXCL) })
	if err != nil {
		return &os.LinkError{Op: "rename", Old: from, New: to, Err: err}
	}
	return nil
}

// RenameExclusive renames from to to, failing with EEXIST when to exists.
func RenameExclusive(from, to string) error {
	err := Retry(func() error { return unix.RenamexNp(from, to, unix.RENAME_EXCL) })
	if err != nil {
		return &os.LinkError{Op: "rename", Old: from, New: to, Err: err}
	}
	return nil
}
