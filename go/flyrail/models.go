package flyrail

import (
	"slices"
	"strings"
	"unicode/utf8"
)

// BundleIdentity is a bundle's id and its opaque version label.
type BundleIdentity struct {
	id      string
	version string
}

// NewBundleIdentity returns a validated bundle identity.
func NewBundleIdentity(id, version string) (BundleIdentity, error) {
	if err := validateIdentifier(id, "bundle id"); err != nil {
		return BundleIdentity{}, err
	}
	if !utf8.ValidString(version) {
		return BundleIdentity{}, newValueError("version must contain Unicode scalar values")
	}
	if err := validateVersion(version); err != nil {
		return BundleIdentity{}, err
	}
	return BundleIdentity{id: id, version: version}, nil
}

// ID returns the bundle id.
func (i BundleIdentity) ID() string { return i.id }

// Version returns the version label.
func (i BundleIdentity) Version() string { return i.version }

// SkillSpec is one schema-1 skill: its name, source path and declared executable files.
type SkillSpec struct {
	name        string
	path        string
	executables []string
}

// NewSkillSpec returns a validated skill specification.
func NewSkillSpec(name, path string, executables []string) (SkillSpec, error) {
	if err := validateIdentifier(name, "skill name"); err != nil {
		return SkillSpec{}, err
	}
	if err := validateRelativePath(path, "skill path"); err != nil {
		return SkillSpec{}, err
	}
	if path[strings.LastIndexByte(path, '/')+1:] != name {
		return SkillSpec{}, newValueError("skill name must match its containing directory")
	}
	seen := make(map[string]struct{}, len(executables))
	for _, executable := range executables {
		if err := validateRelativePath(executable, "executable path"); err != nil {
			return SkillSpec{}, err
		}
		key := portablePathKey(executable)
		if _, duplicate := seen[key]; duplicate {
			return SkillSpec{}, newValueError("executable paths must be unique across portable filesystems")
		}
		seen[key] = struct{}{}
	}
	return SkillSpec{name: name, path: path, executables: slices.Clone(executables)}, nil
}

// Name returns the skill name.
func (s SkillSpec) Name() string { return s.name }

// Path returns the skill's source path relative to the bundle root.
func (s SkillSpec) Path() string { return s.path }

// Executables returns the declared executable paths relative to the skill directory.
func (s SkillSpec) Executables() []string { return slices.Clone(s.executables) }

// BundleEntry is one file or directory of a bundle snapshot.
type BundleEntry struct {
	Path       string // portable relative path
	Data       []byte // file bytes; nil for a directory
	Dir        bool   // a directory entry; Data must be nil and Executable false
	Executable bool   // declared executable intent; physical modes never set it
}

func newEntry(path string, data []byte, dir, executable bool) (BundleEntry, error) {
	entry := BundleEntry{Path: path, Data: data, Dir: dir, Executable: executable}
	if err := validateEntry(entry); err != nil {
		return BundleEntry{}, err
	}
	return copyEntry(entry), nil
}

func directoryEntry(path string) (BundleEntry, error) {
	return newEntry(path, nil, true, false)
}

func validateEntry(entry BundleEntry) error {
	if err := validateRelativePath(entry.Path, "entry path"); err != nil {
		return err
	}
	if entry.Dir && entry.Executable {
		return newValueError("a directory cannot have executable file intent")
	}
	if entry.Dir && entry.Data != nil {
		return newValueError("a directory cannot have file data")
	}
	return nil
}

func copyEntry(entry BundleEntry) BundleEntry {
	if entry.Dir {
		entry.Data = nil
	} else {
		entry.Data = append([]byte{}, entry.Data...)
	}
	return entry
}

func copyEntries(entries []BundleEntry) []BundleEntry {
	if entries == nil {
		return nil
	}
	copied := make([]BundleEntry, len(entries))
	for index, entry := range entries {
		copied[index] = copyEntry(entry)
	}
	return copied
}

func sortEntries(entries []BundleEntry) {
	slices.SortFunc(entries, func(a, b BundleEntry) int { return strings.Compare(a.Path, b.Path) })
}
