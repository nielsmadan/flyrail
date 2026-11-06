//go:build !darwin && !linux

package sysx

import "syscall"

// ErrnoText returns the platform's description of errno.
func ErrnoText(errno syscall.Errno) string {
	return errno.Error()
}
