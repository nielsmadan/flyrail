//go:build darwin || linux

package sysx

import "golang.org/x/sys/unix"

// IsExecutable reports whether path, following links, is a regular file the real user may execute.
func IsExecutable(path string) bool {
	st, err := RawStat(path)
	if err != nil || st.Mode&unix.S_IFMT != unix.S_IFREG {
		return false
	}
	return Retry(func() error { return unix.Access(path, unix.X_OK) }) == nil
}
