package main

import (
	"errors"
	"io/fs"
	"os"
	"path/filepath"
	"slices"
	"strings"
)

const maxSymlinks = 40

// Resolving links lets aliases share one state store.
func resolvePath(path string) (string, error) {
	if !filepath.IsAbs(path) {
		dir, err := os.Getwd()
		if err != nil {
			return "", err
		}
		path = dir + string(filepath.Separator) + path
	}
	pending := components(path)
	resolved := string(filepath.Separator)
	links := 0
	for len(pending) > 0 {
		component := pending[0]
		pending = pending[1:]
		if component == ".." {
			resolved = filepath.Dir(resolved)
			continue
		}
		candidate := filepath.Join(resolved, component)
		info, err := os.Lstat(candidate)
		switch {
		case err == nil && info.Mode()&fs.ModeSymlink != 0:
			links++
			if links > maxSymlinks {
				return "", &fs.PathError{Op: "resolve", Path: path, Err: errors.New("too many symbolic links")}
			}
			target, err := os.Readlink(candidate)
			if err != nil {
				return "", err
			}
			if filepath.IsAbs(target) {
				resolved = string(filepath.Separator)
			}
			pending = append(components(target), pending...)
		case err == nil || errors.Is(err, fs.ErrNotExist):
			resolved = candidate
		default:
			return "", err
		}
	}
	return resolved, nil
}

func components(path string) []string {
	return slices.DeleteFunc(strings.Split(path, string(filepath.Separator)), func(part string) bool {
		return part == "" || part == "."
	})
}
