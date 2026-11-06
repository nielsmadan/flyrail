//go:build darwin || linux

package sysx

import (
	"strings"

	"golang.org/x/sys/unix"
)

const maxSymlinks = 32

func resolvePhysically(path string) (string, error) {
	pending := path
	if !strings.HasPrefix(path, "/") {
		cwd, err := RetryValue(unix.Getwd)
		if err != nil {
			return "", err
		}
		pending = cwd + "/" + path
	}
	components := strings.Split(pending, "/")
	resolved := ""
	links := 0
	for len(components) > 0 {
		name := components[0]
		components = components[1:]
		switch name {
		case "", ".":
			continue
		case "..":
			resolved = resolved[:max(strings.LastIndexByte(resolved, '/'), 0)]
			continue
		}
		candidate := resolved + "/" + name
		var st unix.Stat_t
		if err := Retry(func() error { return unix.Lstat(candidate, &st) }); err != nil {
			return "", err
		}
		if st.Mode&unix.S_IFMT != unix.S_IFLNK {
			resolved = candidate
			continue
		}
		links++
		if links > maxSymlinks {
			return "", unix.ELOOP
		}
		target, err := readLink(candidate)
		if err != nil {
			return "", err
		}
		if strings.HasPrefix(target, "/") {
			resolved = ""
		}
		components = append(strings.Split(target, "/"), components...)
	}
	return resolved, nil
}

func readLink(path string) (string, error) {
	buffer := make([]byte, unix.PathMax)
	count, err := RetryValue(func() (int, error) { return unix.Readlink(path, buffer) })
	if err != nil {
		return "", err
	}
	if count == len(buffer) {
		return "", unix.ENAMETOOLONG
	}
	return string(buffer[:count]), nil
}
