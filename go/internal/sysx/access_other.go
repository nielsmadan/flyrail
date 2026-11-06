//go:build !darwin && !linux

package sysx

import "os"

// IsExecutable reports whether path, following links, is a regular file.
func IsExecutable(path string) bool {
	info, err := os.Stat(path)
	return err == nil && info.Mode().IsRegular()
}
