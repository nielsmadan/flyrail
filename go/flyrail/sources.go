package flyrail

import (
	"context"
	"errors"
	"io/fs"
	"maps"
	"os"
	"slices"
	"strings"
	"syscall"
	"unicode/utf8"

	"github.com/nielsmadan/flyrail/go/internal/sysx"
	"github.com/nielsmadan/flyrail/go/internal/unicodex"
	"github.com/nielsmadan/flyrail/go/internal/zipx"
)

const (
	archiveModeTypeMask  = 0o170000
	archiveModeRegular   = 0o100000
	archiveModeDirectory = 0o040000
	reparsePointFlag     = 0x400
	unixCreateSystem     = 3
)

type bundleSource interface {
	readManifest() ([]byte, error)
	readFile(path string) ([]byte, error)
	treeEntries(path, name string, executables []string) ([]BundleEntry, error)
	finishLoading() error
}

func displayPath(path string) string {
	return unicodex.Lossy(path)
}

func directoryChildPath(parent, name string) string {
	if strings.HasSuffix(parent, "/") {
		return parent + name
	}
	return parent + "/" + name
}

type orderedMap[K comparable, V any] struct {
	keys   []K
	values map[K]V
}

func newOrderedMap[K comparable, V any]() orderedMap[K, V] {
	return orderedMap[K, V]{values: make(map[K]V)}
}

func (m *orderedMap[K, V]) get(key K) (V, bool) {
	value, ok := m.values[key]
	return value, ok
}

func (m *orderedMap[K, V]) set(key K, value V) {
	if _, ok := m.values[key]; !ok {
		m.keys = append(m.keys, key)
	}
	m.values[key] = value
}

type matchKey struct {
	path     string
	matching string
}

type directoryChild struct {
	path string
	name string
}

type directorySource struct {
	ctx         context.Context
	root        string
	observed    orderedMap[string, sysx.Signature]
	directories orderedMap[string, []string]
	matches     orderedMap[matchKey, []string]
	listed      map[string]struct{}
}

func changedWhile(action, path string) error {
	return invalidBundle("bundle source changed while " + action + ": " + displayPath(path))
}

func ordinary(signature sysx.Signature, path string) error {
	if !signature.IsRegular() && !signature.IsDir() {
		return invalidBundle("source symlinks and special files are not supported: " + displayPath(path))
	}
	return nil
}

func newDirectorySource(ctx context.Context, root string) (*directorySource, error) {
	if err := ctx.Err(); err != nil {
		return nil, contextBundleError(err)
	}
	signature, err := sysx.Lstat(root)
	if err != nil {
		return nil, ioBundleError(err)
	}
	if err := ordinary(signature, root); err != nil {
		return nil, err
	}
	resolved, err := sysx.Canonicalize(root)
	if errors.Is(err, fs.ErrNotExist) {
		return nil, changedWhile("loading", root)
	}
	if err != nil {
		return nil, ioBundleError(err)
	}
	resolvedSignature, err := sysx.Lstat(resolved)
	if err != nil {
		return nil, ioBundleError(err)
	}
	if resolvedSignature != signature {
		return nil, changedWhile("loading", root)
	}
	source := &directorySource{
		ctx:         ctx,
		root:        resolved,
		observed:    newOrderedMap[string, sysx.Signature](),
		directories: newOrderedMap[string, []string](),
		matches:     newOrderedMap[matchKey, []string](),
		listed:      make(map[string]struct{}),
	}
	observed, err := source.observe(resolved)
	if err != nil {
		return nil, err
	}
	if !observed.IsDir() {
		return nil, invalidBundle("bundle root must be a directory")
	}
	if _, err := source.directory(resolved, "", false); err != nil {
		return nil, err
	}
	return source, nil
}

func (s *directorySource) observe(path string) (sysx.Signature, error) {
	signature, err := sysx.Lstat(path)
	if err != nil {
		if errors.Is(err, fs.ErrNotExist) {
			_, seen := s.observed.get(path)
			_, listed := s.listed[path]
			if seen || listed {
				return sysx.Signature{}, changedWhile("loading", path)
			}
		}
		return sysx.Signature{}, ioBundleError(err)
	}
	if err := ordinary(signature, path); err != nil {
		return sysx.Signature{}, err
	}
	if previous, ok := s.observed.get(path); ok && previous != signature {
		return sysx.Signature{}, changedWhile("loading", path)
	}
	s.observed.set(path, signature)
	return signature, nil
}

func (s *directorySource) directory(path, matching string, hasMatching bool) ([]directoryChild, error) {
	if err := s.ctx.Err(); err != nil {
		return nil, contextBundleError(err)
	}
	listing, err := os.ReadDir(path)
	if errors.Is(err, fs.ErrNotExist) {
		return nil, changedWhile("loading", path)
	}
	if err != nil {
		return nil, ioBundleError(err)
	}
	children := make([]directoryChild, 0, len(listing))
	for _, entry := range listing {
		children = append(children, directoryChild{path: directoryChildPath(path, entry.Name()), name: entry.Name()})
	}
	if hasMatching {
		key := portablePathKey(matching)
		children = slices.DeleteFunc(children, func(child directoryChild) bool {
			return !utf8.ValidString(child.name) || portablePathKey(child.name) != key
		})
	}
	portable := make(map[string]struct{}, len(children))
	for _, child := range children {
		if utf8.ValidString(child.name) {
			key := portablePathKey(child.name)
			if _, collision := portable[key]; collision {
				return nil, invalidBundle("portable source name collision: " + displayPath(child.path))
			}
			portable[key] = struct{}{}
		}
		s.listed[child.path] = struct{}{}
	}
	names := make([]string, len(children))
	for index, child := range children {
		names[index] = child.name
	}
	slices.Sort(names)
	if hasMatching {
		key := matchKey{path: path, matching: matching}
		if previous, ok := s.matches.get(key); ok && !slices.Equal(previous, names) {
			return nil, changedWhile("loading", path)
		}
		s.matches.set(key, names)
	} else {
		if previous, ok := s.directories.get(path); ok && !slices.Equal(previous, names) {
			return nil, changedWhile("loading", path)
		}
		s.directories.set(path, names)
	}
	return children, nil
}

func (s *directorySource) find(relative string) (string, error) {
	parent := s.root
	for part := range strings.SplitSeq(relative, "/") {
		if _, err := s.observe(parent); err != nil {
			return "", err
		}
		matches, err := s.directory(parent, part, true)
		if err != nil {
			return "", err
		}
		if len(matches) != 1 || matches[0].name != part {
			return "", invalidBundle("missing, case-mismatched or ambiguous source path: " + relative)
		}
		parent = matches[0].path
		if _, err := s.observe(parent); err != nil {
			return "", err
		}
	}
	return parent, nil
}

func (s *directorySource) read(path string) ([]byte, error) {
	before, err := s.observe(path)
	if err != nil {
		return nil, err
	}
	if !before.IsRegular() {
		return nil, invalidBundle("expected a source file: " + displayPath(path))
	}
	data, err := s.snapshotFile(path, before, -1)
	if err != nil {
		return nil, err
	}
	if _, err := s.observe(path); err != nil {
		return nil, err
	}
	return data, nil
}

func (s *directorySource) snapshotFile(path string, before sysx.Signature, limit int64) ([]byte, error) {
	if err := s.ctx.Err(); err != nil {
		return nil, contextBundleError(err)
	}
	fd, err := sysx.OpenNoFollow(path)
	if errors.Is(err, fs.ErrNotExist) || errors.Is(err, syscall.ELOOP) {
		return nil, changedWhile("opening", path)
	}
	if err != nil {
		return nil, ioBundleError(err)
	}
	defer func() { _ = sysx.Close(fd) }()
	opened, err := sysx.Fstat(fd, path)
	if err != nil {
		return nil, ioBundleError(err)
	}
	if opened != before {
		return nil, changedWhile("opening", path)
	}
	if limit >= 0 && opened.Size > limit {
		return nil, archiveBundleError(path, zipx.ArchiveTooLarge())
	}
	data, err := sysx.ReadAll(fd, path, limit)
	if err != nil {
		return nil, ioBundleError(err)
	}
	if limit >= 0 && int64(len(data)) > limit {
		return nil, archiveBundleError(path, zipx.ArchiveTooLarge())
	}
	after, err := sysx.Fstat(fd, path)
	if err != nil {
		return nil, ioBundleError(err)
	}
	if after != opened {
		return nil, changedWhile("reading", path)
	}
	return data, nil
}

func (s *directorySource) readManifest() ([]byte, error) {
	path, err := s.find("flyrail.json")
	if err != nil {
		return nil, err
	}
	return s.read(path)
}

func (s *directorySource) readFile(relative string) ([]byte, error) {
	path, err := s.find(relative)
	if err != nil {
		return nil, err
	}
	return s.read(path)
}

func (s *directorySource) treeEntries(path, name string, executables []string) ([]BundleEntry, error) {
	root, err := s.find(path)
	if err != nil {
		return nil, err
	}
	signature, err := s.observe(root)
	if err != nil {
		return nil, err
	}
	if !signature.IsDir() {
		return nil, invalidBundle("skill source must be a directory: " + path)
	}
	var entries []BundleEntry
	if err := s.walk(root, name, "", executables, &entries); err != nil {
		return nil, err
	}
	return entries, nil
}

func (s *directorySource) walk(directory, relative, rootRelative string, executables []string, entries *[]BundleEntry) error {
	if _, err := s.observe(directory); err != nil {
		return err
	}
	entry, err := directoryEntry(relative)
	if err != nil {
		return asBundleError(err)
	}
	*entries = append(*entries, entry)
	children, err := s.directory(directory, "", false)
	if err != nil {
		return err
	}
	for _, child := range children {
		if !utf8.ValidString(child.name) {
			return invalidBundle("source name must contain NFC-normalized Unicode scalar values")
		}
		if err := validateRelativePath(child.name, "source name"); err != nil {
			return asBundleError(err)
		}
		entryPath := relative + "/" + child.name
		rootPath := child.name
		if rootRelative != "" {
			rootPath = rootRelative + "/" + child.name
		}
		signature, err := s.observe(child.path)
		if err != nil {
			return err
		}
		if signature.IsDir() {
			if err := s.walk(child.path, entryPath, rootPath, executables, entries); err != nil {
				return err
			}
			continue
		}
		data, err := s.read(child.path)
		if err != nil {
			return err
		}
		entry, err := newEntry(entryPath, data, false, slices.Contains(executables, rootPath))
		if err != nil {
			return asBundleError(err)
		}
		*entries = append(*entries, entry)
	}
	if _, err := s.observe(directory); err != nil {
		return err
	}
	_, err = s.directory(directory, "", false)
	return err
}

func (s *directorySource) finish() error {
	for _, path := range slices.Clone(s.observed.keys) {
		if _, err := s.observe(path); err != nil {
			return err
		}
	}
	for _, path := range slices.Clone(s.directories.keys) {
		if _, err := s.directory(path, "", false); err != nil {
			return err
		}
	}
	for _, key := range slices.Clone(s.matches.keys) {
		if _, err := s.directory(key.path, key.matching, true); err != nil {
			return err
		}
	}
	return nil
}

func (s *directorySource) finishLoading() error {
	return s.finish()
}

type zipEntry struct {
	file  bool
	index int
}

type zipSource struct {
	archive     *zipx.Archive
	archivePath string
	root        string
	entries     map[string]zipEntry
	paths       []string
}

func (s *zipSource) member(path string) string {
	return s.root + "/" + path
}

func newZipSource(archive *zipx.Archive, rootValue, archivePath string) (*zipSource, error) {
	root := strings.TrimRight(rootValue, "/")
	rootKey := portablePathKey(root)
	related := func(key string) bool {
		return key == rootKey || strings.HasPrefix(key, rootKey+"/") || strings.HasPrefix(rootKey, key+"/")
	}
	entries := make(map[string]zipEntry)
	explicit := make(map[string]struct{})
	portable := make(map[string]string)
	for index, member := range archive.Members() {
		name := strings.TrimRight(member.Filename, "/")
		selected := related(portablePathKey(name))
		for _, path := range ancestors(name) {
			key := portablePathKey(path)
			if !related(key) {
				continue
			}
			if err := validateRelativePath(path, "archive path"); err != nil {
				return nil, asBundleError(err)
			}
			if previous, ok := portable[key]; ok && previous != path {
				return nil, invalidBundle("portable archive name collision: " + path)
			}
			portable[key] = path
		}
		if !selected {
			continue
		}
		if strings.Contains(member.OrigFilename, "\x00") {
			return nil, invalidBundle("ambiguous archive entry: " + archiveRepr(member.OrigFilename))
		}
		if err := validateRelativePath(strings.TrimRight(member.OrigFilename, "/"), "archive path"); err != nil {
			return nil, asBundleError(err)
		}
		if member.OrigFilename != member.Filename {
			return nil, invalidBundle("ambiguous archive entry: " + archiveRepr(member.OrigFilename))
		}
		expected := name
		if member.IsDir {
			expected += "/"
		}
		if _, duplicate := explicit[name]; duplicate || member.Filename != expected {
			return nil, invalidBundle("duplicate or ambiguous archive entry: " + member.Filename)
		}
		explicit[name] = struct{}{}
		kind := uint32(0)
		if member.CreateSystem == unixCreateSystem {
			kind = (member.ExternalAttr >> 16) & archiveModeTypeMask
		}
		expectedKind := uint32(archiveModeRegular)
		if member.IsDir {
			expectedKind = archiveModeDirectory
		}
		if kind != 0 && kind != expectedKind {
			return nil, invalidBundle("archive symlinks and special files are not supported: " + name)
		}
		if member.ExternalAttr&reparsePointFlag != 0 {
			return nil, invalidBundle("archive reparse points are not supported: " + name)
		}
		for _, path := range ancestors(name) {
			isFile := path == name && !member.IsDir
			if previous, ok := entries[path]; ok && (isFile || previous.file) {
				return nil, invalidBundle("archive file/directory collision: " + path)
			}
			entries[path] = zipEntry{file: isFile, index: index}
		}
	}
	if entry, ok := entries[root]; !ok || entry.file {
		return nil, invalidBundle("package resource must be an existing directory")
	}
	paths := slices.Sorted(maps.Keys(entries))
	return &zipSource{archive: archive, archivePath: archivePath, root: root, entries: entries, paths: paths}, nil
}

func (s *zipSource) readMember(index int) ([]byte, error) {
	data, err := s.archive.Read(index)
	if err != nil {
		return nil, archiveBundleError(s.archivePath, err)
	}
	return data, nil
}

func (s *zipSource) readManifest() ([]byte, error) {
	entry, ok := s.entries[s.member("flyrail.json")]
	if !ok || !entry.file {
		return nil, invalidBundle("package resource must contain a flyrail.json file")
	}
	return s.readMember(entry.index)
}

func (s *zipSource) readFile(path string) ([]byte, error) {
	entry, ok := s.entries[s.member(path)]
	if !ok || !entry.file {
		return nil, invalidBundle("expected a source file: " + path)
	}
	return s.readMember(entry.index)
}

func (s *zipSource) treeEntries(path, name string, executables []string) ([]BundleEntry, error) {
	root := s.member(path)
	if entry, ok := s.entries[root]; !ok || entry.file {
		return nil, invalidBundle("skill source must be an existing directory: " + path)
	}
	top, err := directoryEntry(name)
	if err != nil {
		return nil, asBundleError(err)
	}
	result := []BundleEntry{top}
	for _, entryPath := range s.paths {
		relative, ok := strings.CutPrefix(entryPath, root+"/")
		if !ok {
			continue
		}
		entry := s.entries[entryPath]
		var data []byte
		if entry.file {
			if data, err = s.readMember(entry.index); err != nil {
				return nil, err
			}
		}
		bundleEntry, err := newEntry(name+"/"+relative, data, !entry.file, slices.Contains(executables, relative))
		if err != nil {
			return nil, asBundleError(err)
		}
		result = append(result, bundleEntry)
	}
	return result, nil
}

func (s *zipSource) finishLoading() error {
	return nil
}

func archiveRepr(text string) string {
	var out strings.Builder
	out.WriteByte('\'')
	for _, character := range text {
		switch {
		case character == '\\':
			out.WriteString(`\\`)
		case character == '\'':
			out.WriteString(`\'`)
		case character == '\n':
			out.WriteString(`\n`)
		case character == '\r':
			out.WriteString(`\r`)
		case character == '\t':
			out.WriteString(`\t`)
		case character < 0x20:
			out.WriteString(`\x`)
			out.WriteByte("0123456789abcdef"[character>>4])
			out.WriteByte("0123456789abcdef"[character&0xF])
		default:
			out.WriteRune(character)
		}
	}
	out.WriteByte('\'')
	return out.String()
}

type memorySource struct {
	manifest []byte
	entries  []BundleEntry
	index    map[string]int
}

func newMemorySource(manifest []byte, entries []BundleEntry) (*memorySource, error) {
	for _, entry := range entries {
		if err := validateEntry(entry); err != nil {
			return nil, asBundleError(err)
		}
	}
	snapshot, err := snapshotEntries(entries)
	if err != nil {
		return nil, asBundleError(err)
	}
	index := make(map[string]int, len(snapshot))
	for position, entry := range snapshot {
		index[entry.Path] = position
	}
	return &memorySource{manifest: slices.Clone(manifest), entries: snapshot, index: index}, nil
}

func snapshotEntries(entries []BundleEntry) ([]BundleEntry, error) {
	selected := make([]BundleEntry, 0, len(entries))
	positions := make(map[string]int, len(entries))
	spellings := make(map[string]string)
	for _, entry := range entries {
		if _, duplicate := positions[entry.Path]; duplicate {
			return nil, newValueError("duplicate entry path")
		}
		for _, path := range ancestors(entry.Path) {
			key := portablePathKey(path)
			if previous, ok := spellings[key]; ok && previous != path {
				return nil, newValueError("portable entry path collision")
			}
			spellings[key] = path
		}
		positions[entry.Path] = len(selected)
		selected = append(selected, copyEntry(entry))
	}
	explicit := len(selected)
	for position := range explicit {
		entry := selected[position]
		for _, path := range ancestors(entry.Path) {
			if path == entry.Path {
				continue
			}
			if existing, ok := positions[path]; ok {
				if !selected[existing].Dir {
					return nil, newValueError("entry file/directory collision")
				}
				continue
			}
			positions[path] = len(selected)
			selected = append(selected, BundleEntry{Path: path, Dir: true})
		}
	}
	sortEntries(selected)
	return selected, nil
}

func (s *memorySource) readManifest() ([]byte, error) {
	return slices.Clone(s.manifest), nil
}

func (s *memorySource) readFile(path string) ([]byte, error) {
	position, ok := s.index[path]
	if !ok || s.entries[position].Dir {
		return nil, invalidBundle("expected a source file: " + path)
	}
	return slices.Clone(s.entries[position].Data), nil
}

func (s *memorySource) treeEntries(path, name string, executables []string) ([]BundleEntry, error) {
	if position, ok := s.index[path]; !ok || !s.entries[position].Dir {
		return nil, invalidBundle("skill source must be an existing directory: " + path)
	}
	top, err := directoryEntry(name)
	if err != nil {
		return nil, asBundleError(err)
	}
	result := []BundleEntry{top}
	for _, entry := range s.entries {
		relative, ok := strings.CutPrefix(entry.Path, path+"/")
		if !ok {
			continue
		}
		bundleEntry, err := newEntry(name+"/"+relative, entry.Data, entry.Dir, slices.Contains(executables, relative))
		if err != nil {
			return nil, asBundleError(err)
		}
		result = append(result, bundleEntry)
	}
	return result, nil
}

func (s *memorySource) finishLoading() error {
	return nil
}

type fsSource struct {
	fsys fs.FS
}

func fsJoin(parent, name string) string {
	if parent == "." {
		return name
	}
	return parent + "/" + name
}

func (s *fsSource) list(directory string) ([]fs.DirEntry, error) {
	listing, err := fs.ReadDir(s.fsys, directory)
	if err != nil {
		return nil, ioBundleError(err)
	}
	return listing, nil
}

func fsOrdinary(entry fs.DirEntry, path string) error {
	if !entry.IsDir() && !entry.Type().IsRegular() {
		return invalidBundle("source symlinks and special files are not supported: " + path)
	}
	return nil
}

func (s *fsSource) find(relative string) (string, fs.DirEntry, error) {
	parent := "."
	var found fs.DirEntry
	for part := range strings.SplitSeq(relative, "/") {
		listing, err := s.list(parent)
		if err != nil {
			return "", nil, err
		}
		key := portablePathKey(part)
		matches := slices.DeleteFunc(listing, func(entry fs.DirEntry) bool {
			return !utf8.ValidString(entry.Name()) || portablePathKey(entry.Name()) != key
		})
		if len(matches) > 1 {
			return "", nil, invalidBundle("portable source name collision: " + fsJoin(parent, matches[1].Name()))
		}
		if len(matches) != 1 || matches[0].Name() != part {
			return "", nil, invalidBundle("missing, case-mismatched or ambiguous source path: " + relative)
		}
		parent, found = fsJoin(parent, part), matches[0]
		if err := fsOrdinary(found, parent); err != nil {
			return "", nil, err
		}
	}
	return parent, found, nil
}

func (s *fsSource) read(path string, entry fs.DirEntry) ([]byte, error) {
	if !entry.Type().IsRegular() {
		return nil, invalidBundle("expected a source file: " + path)
	}
	data, err := fs.ReadFile(s.fsys, path)
	if err != nil {
		return nil, ioBundleError(err)
	}
	return data, nil
}

func (s *fsSource) readManifest() ([]byte, error) {
	return s.readFile("flyrail.json")
}

func (s *fsSource) readFile(relative string) ([]byte, error) {
	path, entry, err := s.find(relative)
	if err != nil {
		return nil, err
	}
	return s.read(path, entry)
}

func (s *fsSource) treeEntries(path, name string, executables []string) ([]BundleEntry, error) {
	root, entry, err := s.find(path)
	if err != nil {
		return nil, err
	}
	if !entry.IsDir() {
		return nil, invalidBundle("skill source must be a directory: " + path)
	}
	var entries []BundleEntry
	if err := s.walk(root, name, "", executables, &entries); err != nil {
		return nil, err
	}
	return entries, nil
}

func (s *fsSource) walk(directory, relative, rootRelative string, executables []string, entries *[]BundleEntry) error {
	entry, err := directoryEntry(relative)
	if err != nil {
		return asBundleError(err)
	}
	*entries = append(*entries, entry)
	listing, err := s.list(directory)
	if err != nil {
		return err
	}
	portable := make(map[string]struct{}, len(listing))
	for _, child := range listing {
		if utf8.ValidString(child.Name()) {
			key := portablePathKey(child.Name())
			if _, collision := portable[key]; collision {
				return invalidBundle("portable source name collision: " + fsJoin(directory, child.Name()))
			}
			portable[key] = struct{}{}
		}
	}
	for _, child := range listing {
		name := child.Name()
		if !utf8.ValidString(name) {
			return invalidBundle("source name must contain NFC-normalized Unicode scalar values")
		}
		if err := validateRelativePath(name, "source name"); err != nil {
			return asBundleError(err)
		}
		path := fsJoin(directory, name)
		rootPath := name
		if rootRelative != "" {
			rootPath = rootRelative + "/" + name
		}
		if err := fsOrdinary(child, path); err != nil {
			return err
		}
		if child.IsDir() {
			if err := s.walk(path, relative+"/"+name, rootPath, executables, entries); err != nil {
				return err
			}
			continue
		}
		data, err := s.read(path, child)
		if err != nil {
			return err
		}
		fileEntry, err := newEntry(relative+"/"+name, data, false, slices.Contains(executables, rootPath))
		if err != nil {
			return asBundleError(err)
		}
		*entries = append(*entries, fileEntry)
	}
	return nil
}

func (s *fsSource) finishLoading() error {
	return nil
}
