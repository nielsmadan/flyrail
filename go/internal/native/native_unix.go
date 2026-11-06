//go:build darwin || linux

package native

import (
	"errors"
	"strings"
	"syscall"
	"unicode/utf8"

	"golang.org/x/sys/unix"

	"github.com/nielsmadan/flyrail/go/internal/pathx"
	"github.com/nielsmadan/flyrail/go/internal/sysx"
	"github.com/nielsmadan/flyrail/go/internal/unicodex"
)

const maxResolvedLinks = 40

func isDirectory(st *unix.Stat_t) bool {
	return st.Mode&unix.S_IFMT == unix.S_IFDIR
}

func isRegular(st *unix.Stat_t) bool {
	return st.Mode&unix.S_IFMT == unix.S_IFREG
}

func isSymlink(st *unix.Stat_t) bool {
	return st.Mode&unix.S_IFMT == unix.S_IFLNK
}

// EnsurePrivate refuses management storage that its owner does not hold exclusively or that carries foreign security metadata.
func EnsurePrivate(path string) error {
	st, err := sysx.RawLstat(path)
	if err != nil {
		return ioFailure(err, path)
	}
	if st.Mode&0o077 != 0 || st.Uid != uint32(unix.Geteuid()) { //nolint:gosec // effective user ids are non-negative
		return refused(syscall.ENOTSUP, "management storage must be private to its owner", path)
	}
	if _, err := Security(path, false); err != nil {
		return err
	}
	return nil
}

// FileTimes returns the modification time of path, and its birth time on macOS, without following a final symbolic link.
func FileTimes(path string) (Times, error) {
	st, err := sysx.RawLstat(path)
	if err != nil {
		return Times{}, ioFailure(err, path)
	}
	return timesOf(&st), nil
}

func checkSafe(st *unix.Stat_t, path string) error {
	if !isRegular(st) && !isDirectory(st) {
		return unsafePath("resource links and special files are unsupported: "+unicodex.Lossy(path), path)
	}
	if isRegular(st) && st.Nlink != 1 {
		return unsafePath("hard-linked resources are ambiguous: "+unicodex.Lossy(path), path)
	}
	return nil
}

// CanonicalDestination resolves an absolute destination without ".." components to its physical spelling, leaving component validation of argument and result to the caller.
func CanonicalDestination(selected string) (string, error) {
	if selected == "" || strings.IndexByte(selected, 0) >= 0 {
		return "", invalid("resource destination must be a nonempty string path without NUL")
	}
	parts := pathx.Components(selected)
	if len(parts) < 2 || parts[0] != "/" {
		return "", invalid("resource destination cannot be a root or contain traversal")
	}
	parts = parts[1:]
	for _, part := range parts {
		if part == ".." {
			return "", invalid("resource destination cannot be a root or contain traversal")
		}
	}
	finalName := parts[len(parts)-1]
	if !utf8.ValidString(finalName) {
		return "", invalid("resource destination must be a nonempty string path without NUL")
	}
	current := "/"
	var missing []string
	for _, name := range parts[:len(parts)-1] {
		if len(missing) > 0 {
			missing = append(missing, name)
			continue
		}
		candidate := pathx.Join(current, name)
		st, err := sysx.RawLstat(candidate)
		switch {
		case errors.Is(err, syscall.ENOENT):
			missing = append(missing, name)
			continue
		case err != nil:
			return "", ioFailure(err, candidate)
		}
		if err := checkSafe(&st, candidate); err != nil {
			return "", err
		}
		if !isDirectory(&st) {
			return "", unsafePath("resource ancestor must be a directory", candidate)
		}
		current = candidate
	}
	physical, err := PhysicalDirectory(current)
	if err != nil {
		return "", err
	}
	if len(missing) == 0 {
		return CanonicalChild(physical, finalName)
	}
	if !utf8.ValidString(missing[0]) {
		return "", invalid("resource destination must be a nonempty string path without NUL")
	}
	staged, err := CanonicalChild(physical, missing[0])
	if err != nil {
		return "", err
	}
	for _, name := range missing[1:] {
		staged = pathx.Join(staged, name)
	}
	return pathx.Join(staged, finalName), nil
}

// CanonicalChild returns the stored spelling of name inside the physical directory parent, or parent/name when no entry matches its portable key.
func CanonicalChild(parent, name string) (string, error) {
	candidate := pathx.Join(parent, name)
	key := unicodex.PortableKey(name)
	names, err := sysx.ReadDirNames(parent)
	if err != nil {
		return "", ioFailure(err, parent)
	}
	var matches []string
	for _, stored := range names {
		if utf8.ValidString(stored) && unicodex.PortableKey(stored) == key {
			matches = append(matches, stored)
		}
	}
	if len(matches) > 1 {
		return "", unsafePath("portable resource path collision", candidate)
	}
	if len(matches) == 0 {
		return candidate, nil
	}
	physical := pathx.Join(parent, matches[0])
	st, err := sysx.RawLstat(physical)
	if err != nil {
		return "", ioFailure(err, physical)
	}
	if err := checkSafe(&st, physical); err != nil {
		return "", err
	}
	if matches[0] != name {
		alias, err := sysx.RawLstat(candidate)
		switch {
		case errors.Is(err, syscall.ENOENT):
			return "", unsafePath("resource path has a conflicting portable spelling", candidate)
		case err != nil:
			return "", ioFailure(err, candidate)
		}
		if alias.Dev != st.Dev || alias.Ino != st.Ino {
			return "", unsafePath("resource path alias is ambiguous", candidate)
		}
	}
	return physical, nil
}

func lenientResolve(path string) string {
	pending := pathx.Components(path)
	links := 0
	resolved := "/"
	for len(pending) > 0 {
		name := pending[0]
		pending = pending[1:]
		switch name {
		case "/":
			resolved = "/"
			continue
		case ".":
			continue
		case "..":
			if up, ok := pathx.NormalizedParent(resolved); ok {
				resolved = up
			}
			continue
		}
		candidate := pathx.Join(resolved, name)
		if links < maxResolvedLinks {
			if st, err := sysx.RawLstat(candidate); err == nil && isSymlink(&st) {
				if target, err := sysx.Readlink(candidate); err == nil {
					links++
					pending = append(linkComponents(target), pending...)
					continue
				}
			}
		}
		resolved = candidate
	}
	return resolved
}

// Drops a leading "." as Rust's owned components do.
func linkComponents(target string) []string {
	parts := pathx.Components(target)
	if len(parts) > 0 && parts[0] == "." {
		return parts[1:]
	}
	return parts
}

// PhysicalLocation resolves path to a physical location whose deepest existing directory is canonical and whose missing tail is kept.
func PhysicalLocation(path string) (string, error) {
	if path == "" {
		return "", &Error{Kind: KindIO, Message: "cannot make an empty path absolute"}
	}
	absolute := path
	if !strings.HasPrefix(path, "/") {
		current, err := sysx.Getwd()
		if err != nil {
			return "", ioFailure(err, path)
		}
		absolute = pathx.Join(current, path)
	}
	existing := lenientResolve(absolute)
	var missing []string
	for {
		if st, err := sysx.RawStat(existing); err == nil && isDirectory(&st) {
			break
		}
		name, ok := pathx.FileName(existing)
		if !ok {
			break
		}
		missing = append([]string{name}, missing...)
		up, _ := pathx.NormalizedParent(existing)
		existing = up
	}
	physical, err := PhysicalDirectory(existing)
	if err != nil {
		return "", err
	}
	for _, name := range missing {
		physical = pathx.Join(physical, name)
	}
	return physical, nil
}

// TransactionEntries returns every transaction directory inside the whole-tree resource destination, refusing links and special files.
func TransactionEntries(destination string) ([]string, error) {
	var entries []string
	pending := []string{destination}
	for len(pending) > 0 {
		directory := pending[len(pending)-1]
		pending = pending[:len(pending)-1]
		st, err := sysx.RawLstat(directory)
		if err != nil {
			return nil, ioFailure(err, directory)
		}
		if err := checkSafe(&st, directory); err != nil {
			return nil, err
		}
		if !isDirectory(&st) {
			return nil, unsafePath("whole-tree resource must be a directory", directory)
		}
		names, err := sysx.ReadDirNames(directory)
		if err != nil {
			return nil, ioFailure(err, directory)
		}
		for _, name := range names {
			child := pathx.Join(directory, name)
			if ManagementName(unicodex.Lossy(name)) {
				entries = append(entries, child)
				continue
			}
			st, err := sysx.RawLstat(child)
			if err != nil {
				return nil, ioFailure(err, child)
			}
			if err := checkSafe(&st, child); err != nil {
				return nil, err
			}
			if isDirectory(&st) {
				pending = append(pending, child)
			}
		}
	}
	return entries, nil
}

// ManagedSpelling refuses a management directory whose stored spelling, type or link count makes it ambiguous.
func ManagedSpelling(stateRoot string) error {
	name, hasName := pathx.FileName(stateRoot)
	directory, hasParent := pathx.NormalizedParent(stateRoot)
	if !hasName || !hasParent {
		return nil
	}
	st, err := sysx.RawLstat(directory)
	switch {
	case errors.Is(err, syscall.ENOENT):
	case err != nil:
		return ioFailure(err, directory)
	case !isDirectory(&st):
		return unsafePath("resource ancestor is not an ordinary directory", directory)
	}
	key := unicodex.PortableKey(unicodex.Lossy(name))
	names, err := sysx.ReadDirNames(directory)
	if err != nil && !errors.Is(err, syscall.ENOENT) {
		return ioFailure(err, directory)
	}
	var matches []string
	for _, stored := range names {
		if utf8.ValidString(stored) && unicodex.PortableKey(stored) == key {
			matches = append(matches, stored)
		}
	}
	if len(matches) > 1 {
		return unsafePath("portable filename collision", stateRoot)
	}
	if len(matches) == 1 && matches[0] != name {
		return unsafePath("managed directory spelling mismatch", stateRoot)
	}
	if root, err := sysx.RawLstat(stateRoot); err == nil {
		if !isRegular(&root) && !isDirectory(&root) || isRegular(&root) && root.Nlink != 1 {
			return unsafePath("managed symlinks, reparse points and special files are unsupported", stateRoot)
		}
		if !isDirectory(&root) {
			return unsafePath("expected a directory", stateRoot)
		}
	}
	return nil
}

// SameMount reports whether first and second live on one device and, on Linux, one mount entry.
func SameMount(first, second string) (bool, error) {
	left, err := device(first)
	if err != nil {
		return false, err
	}
	right, err := device(second)
	if err != nil {
		return false, err
	}
	if left != right {
		return false, nil
	}
	table, ok := mountTable()
	return !ok || SharedMount(table, first, second), nil
}

func device(path string) (uint64, error) {
	candidate, ok := path, true
	for ok {
		st, err := sysx.RawStat(candidate)
		switch {
		case err == nil:
			return sysx.DeviceNumber(&st), nil
		case !errors.Is(err, syscall.ENOENT):
			return 0, ioFailure(err, candidate)
		}
		candidate, ok = pathx.NormalizedParent(candidate)
	}
	return 0, ioFailure(syscall.ENOENT, path)
}
