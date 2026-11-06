//go:build darwin || linux

package sysx

import (
	"strconv"
	"syscall"
)

//go:generate python3 -I ../../scripts/generate_errno_text.py

// ErrnoText returns the C library's strerror wording for errno, as Python's os.strerror.
func ErrnoText(errno syscall.Errno) string {
	if int(errno) >= 0 && int(errno) < len(errnoText) {
		return errnoText[errno]
	}
	return unknownErrnoPrefix + strconv.Itoa(int(errno))
}
