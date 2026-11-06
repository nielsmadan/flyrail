//go:build !darwin && !linux

package sysx

import (
	"errors"
	"os"
)

// RenameatExclusive reports that exclusive rename is unsupported on this platform.
func RenameatExclusive(_ int, from string, _ int, to string) error {
	return &os.LinkError{Op: "rename", Old: from, New: to, Err: errors.ErrUnsupported}
}

// RenameExclusive reports that exclusive rename is unsupported on this platform.
func RenameExclusive(from, to string) error {
	return &os.LinkError{Op: "rename", Old: from, New: to, Err: errors.ErrUnsupported}
}
