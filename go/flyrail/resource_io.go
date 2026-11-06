package flyrail

import (
	"context"
	"errors"
	"math"
	"slices"
	"strconv"
	"strings"
	"syscall"
	"time"
	"unicode/utf8"

	"github.com/nielsmadan/flyrail/go/flyrail/store"
	"github.com/nielsmadan/flyrail/go/internal/native"
	"github.com/nielsmadan/flyrail/go/internal/pathx"
	"github.com/nielsmadan/flyrail/go/internal/seams"
	"github.com/nielsmadan/flyrail/go/internal/sysx"
)

// The resource budgets: observation and staging refuse anything larger, as Swift's NativeLimits do.
const (
	maxResourceFileBytes  = 64 << 20
	maxResourceTotalBytes = 256 << 20
	maxResourceNodes      = 10000
	maxResourceDepth      = 128
)

func failure(code ErrorCode, message, path string) *TargetError {
	return newTargetError(code, message, path, 0, nil)
}

func limitFailure(message, path string) *TargetError {
	errno := syscall.ENOTSUP
	return newTargetError(ioCode(errno), sysx.PosixPathRefusalMessage(errno, message, path), path, errno, nil)
}

func notFound(err error) bool {
	return errors.Is(err, syscall.ENOENT)
}

func errorDetail(err error) *TargetError {
	authority, ok := errors.AsType[*AuthorityError](err)
	if !ok {
		return ioFailure(err, "")
	}
	if _, invalid := errors.AsType[*ValueError](authority.Err); invalid {
		return newTargetError(CodeInvalidState, authority.Message, "", 0, authority.Err)
	}
	if errors.Is(authority.Err, ErrUnsafePath) {
		return newTargetError(CodeUnsafePath, authority.Message, authority.Path, 0, authority.Err)
	}
	errno := sysx.Errno(authority.Err)
	return newTargetError(ioCode(errno), authority.Message, authority.Path, errno, authority.Err)
}

func countOf(value uint64) count {
	if value > math.MaxInt64 {
		return count{big: strconv.FormatUint(value, 10)}
	}
	return count{small: int64(value)}
}

func managedMetadata(metadata sysx.Metadata, path string) *TargetError {
	if !metadata.IsRegular() && !metadata.IsDir() || metadata.IsRegular() && metadata.Nlink != 1 {
		return failure(CodeUnsafePath, "managed symlinks, reparse points and special files are unsupported", path)
	}
	return nil
}

func securityOf(path string, ancestorEvidence bool) ([]byte, *TargetError) {
	descriptor, err := native.Security(path, ancestorEvidence)
	if err != nil {
		return nil, nativeFailure(err)
	}
	return descriptor, nil
}

func metadataIdentity(metadata sysx.Metadata, modeMask uint32) fileIdentity {
	return fileIdentity{
		device: countOf(metadata.Dev),
		inode:  countOf(metadata.Ino),
		mode:   count{small: int64(metadata.Mode & modeMask)},
		uid:    count{small: int64(metadata.UID)},
		gid:    count{small: int64(metadata.GID)},
	}
}

func (a ancestor) equal(o ancestor) bool {
	return a.path == o.path && a.fileIdentity == o.fileIdentity && string(a.security) == string(o.security)
}

func ancestorsEqual(a, b []ancestor) bool {
	return slices.EqualFunc(a, b, ancestor.equal)
}

func parentPaths(path string) []string {
	var parents []string
	for current, ok := pathx.Parent(path); ok; current, ok = pathx.Parent(current) {
		parents = append(parents, current)
	}
	slices.Reverse(parents)
	return parents
}

func captureAncestor(parent string) (*ancestor, *TargetError) {
	metadata, err := sysx.LstatMetadata(parent)
	if notFound(err) {
		return nil, nil
	}
	if err != nil {
		return nil, ioFailure(err, parent)
	}
	if failed := managedMetadata(metadata, parent); failed != nil {
		return nil, failed
	}
	if !metadata.IsDir() {
		return nil, failure(CodeUnsafePath, "resource ancestor is not an ordinary directory", parent)
	}
	descriptor, failed := securityOf(parent, true)
	if failed != nil {
		return nil, failed
	}
	captured, err := newAncestor(parent, metadataIdentity(metadata, 0o7777), descriptor)
	if err != nil {
		return nil, failure(CodeInvalidState, err.Error(), parent)
	}
	return &captured, nil
}

func sameCapture(a, b *ancestor) bool {
	if a == nil || b == nil {
		return a == b
	}
	return a.equal(*b)
}

// Captures each ancestor twice and refuses one that changed in between.
func captureAncestors(path string) ([]ancestor, *TargetError) {
	parents := parentPaths(path)
	first := make([]*ancestor, 0, len(parents))
	for _, parent := range parents {
		captured, failed := captureAncestor(parent)
		if failed != nil {
			return nil, failed
		}
		first = append(first, captured)
	}
	for index, parent := range parents {
		captured, failed := captureAncestor(parent)
		if failed != nil {
			return nil, failed
		}
		if !sameCapture(captured, first[index]) {
			return nil, failure(CodeConcurrentChange, "ancestor changed during inspection", parent)
		}
	}
	result := make([]ancestor, 0, len(first))
	for _, captured := range first {
		if captured != nil {
			result = append(result, *captured)
		}
	}
	return result, nil
}

func resourceAncestors(destination string, paths ...string) ([]ancestor, *TargetError) {
	var result []ancestor
	for _, path := range append([]string{destination}, paths...) {
		items, failed := captureAncestors(path)
		if failed != nil {
			return nil, failed
		}
		for _, item := range items {
			if !slices.ContainsFunc(result, item.equal) {
				result = append(result, item)
			}
		}
	}
	return result, nil
}

func readFile(path string, before sysx.Metadata, limit int64, opened func(string)) ([]byte, *TargetError) {
	fd, err := sysx.OpenNoFollow(path)
	switch {
	case notFound(err):
		return nil, failure(CodeConcurrentChange, "file disappeared while opening", path)
	case errors.Is(err, syscall.ELOOP):
		return nil, failure(CodeConcurrentChange, "file changed while opening", path)
	case err != nil:
		return nil, ioFailure(err, path)
	}
	defer func() { _ = sysx.Close(fd) }()
	metadata, err := sysx.FstatMetadata(fd, path)
	if err != nil {
		return nil, ioFailure(err, "")
	}
	if metadata != before {
		return nil, failure(CodeConcurrentChange, "file changed while opening", path)
	}
	if metadata.Size > limit {
		return nil, limitFailure("resource exceeds the byte limit", path)
	}
	if opened != nil {
		opened(path)
	}
	data, err := sysx.ReadAll(fd, path, limit)
	if err != nil {
		return nil, ioFailure(err, "")
	}
	if int64(len(data)) > limit {
		return nil, limitFailure("resource exceeds the byte limit", path)
	}
	completed, err := sysx.FstatMetadata(fd, path)
	if err != nil {
		return nil, ioFailure(err, "")
	}
	if completed != metadata {
		return nil, failure(CodeConcurrentChange, "file changed while reading", path)
	}
	current, err := sysx.LstatMetadata(path)
	switch {
	case notFound(err):
		return nil, failure(CodeConcurrentChange, "path changed during inspection", path)
	case err != nil:
		return nil, ioFailure(err, path)
	case current != metadata:
		return nil, failure(CodeConcurrentChange, "path changed during inspection", path)
	}
	return data, nil
}

type childEntry struct {
	name string
	path string
}

func directoryChildren(path string, expected sysx.Metadata, limit int) ([]childEntry, *TargetError) {
	names, opened, err := sysx.ReadDirNamesNoFollow(path, limit)
	switch {
	case errors.Is(err, syscall.ELOOP) || errors.Is(err, syscall.ENOTDIR):
		return nil, revisionChanged(path)
	case err != nil:
		return nil, ioFailure(err, path)
	case opened.Dev != expected.Dev || opened.Ino != expected.Ino || !opened.IsDir():
		return nil, revisionChanged(path)
	}
	children := make([]childEntry, 0, len(names))
	for _, name := range names {
		child := pathx.Join(path, name)
		if !utf8.ValidString(name) {
			return nil, failure(CodeUnsafePath, "nonportable resource path", child)
		}
		children = append(children, childEntry{name: name, path: child})
	}
	slices.SortFunc(children, func(a, b childEntry) int { return strings.Compare(a.name, b.name) })
	keys := make(map[string]bool, len(children))
	for _, child := range children {
		key := portablePathKey(child.name)
		if keys[key] {
			return nil, failure(CodeUnsafePath, "portable filename collision", child.path)
		}
		keys[key] = true
	}
	return children, nil
}

func childNames(children []childEntry) []string {
	names := make([]string, 0, len(children))
	for _, child := range children {
		names = append(names, child.name)
	}
	return names
}

// security and opened are test seams for the native capture and the file-read pause.
type observation struct {
	nodes      []node
	signatures map[string]sysx.Metadata
	totalBytes int64
	security   func(path string) ([]byte, *TargetError)
	opened     func(path string)
}

func (w *observation) securityOf(path string) ([]byte, *TargetError) {
	if w.security != nil {
		return w.security(path)
	}
	return securityOf(path, false)
}

func (w *observation) visit(path, relative string, depth int) *TargetError {
	if len(w.nodes) >= maxResourceNodes || depth > maxResourceDepth {
		return limitFailure("resource exceeds the node limit", path)
	}
	metadata, err := sysx.LstatMetadata(path)
	if notFound(err) {
		return nil
	}
	if err != nil {
		return ioFailure(err, path)
	}
	if failed := managedMetadata(metadata, path); failed != nil {
		return failed
	}
	w.signatures[path] = metadata
	if metadata.Mode&0o7000 != 0 {
		return failure(CodeUnsafePath, "special permission bits are unsupported", path)
	}
	descriptor, failed := w.securityOf(path)
	if failed != nil {
		return failed
	}
	var data []byte
	if !metadata.IsDir() {
		if data, failed = readFile(path, metadata, min(maxResourceFileBytes, maxResourceTotalBytes-w.totalBytes), w.opened); failed != nil {
			return failed
		}
		w.totalBytes += int64(len(data))
	}
	observed, err := newNode(relative, data, !metadata.IsDir(), metadataIdentity(metadata, 0o777), descriptor)
	if err != nil {
		return failure(CodeInvalidState, err.Error(), path)
	}
	w.nodes = append(w.nodes, observed)
	if metadata.IsDir() {
		children, failed := directoryChildren(path, metadata, maxResourceNodes-len(w.nodes))
		if failed != nil {
			return failed
		}
		if len(children) > maxResourceNodes-len(w.nodes) {
			return limitFailure("resource exceeds the node limit", path)
		}
		for _, child := range children {
			childRelative := child.name
			if relative != "" {
				childRelative = relative + "/" + child.name
			}
			if failed := w.visit(child.path, childRelative, depth+1); failed != nil {
				return failed
			}
		}
		current, failed := directoryChildren(path, metadata, len(children))
		if failed != nil {
			return failed
		}
		if !slices.Equal(childNames(current), childNames(children)) {
			return failure(CodeConcurrentChange, "directory entries changed", path)
		}
	}
	current, err := sysx.LstatMetadata(path)
	if err != nil {
		return ioFailure(err, path)
	}
	if current != metadata {
		return failure(CodeConcurrentChange, "path changed during inspection", path)
	}
	again, failed := w.securityOf(path)
	if failed != nil {
		return failed
	}
	if string(again) != string(descriptor) {
		return failure(CodeConcurrentChange, "security metadata changed", path)
	}
	return nil
}

func nodePath(path string, item node) string {
	if item.path == "" {
		return path
	}
	return pathx.Join(path, item.path)
}

func countMatches(c count, value uint64) bool {
	actual, ok := c.uint64()
	return ok && actual == value
}

func revisionChanged(path string) *TargetError {
	return failure(CodeConcurrentChange, "path changed during inspection", path)
}

func verifyObserved(path string, observed revision, signatures map[string]sysx.Metadata) *TargetError {
	if len(observed.nodes) == 0 {
		_, err := sysx.LstatMetadata(path)
		switch {
		case notFound(err):
			return nil
		case err != nil:
			return ioFailure(err, path)
		}
		return revisionChanged(path)
	}
	var childrenByParent map[string][]string
	for _, item := range observed.nodes {
		selected := nodePath(path, item)
		metadata, err := sysx.LstatMetadata(selected)
		switch {
		case notFound(err):
			return revisionChanged(selected)
		case err != nil:
			return ioFailure(err, selected)
		}
		if failed := managedMetadata(metadata, selected); failed != nil {
			return failed
		}
		recorded, known := signatures[selected]
		unchanged := known && recorded == metadata && countMatches(item.device, metadata.Dev) && countMatches(item.inode, metadata.Ino) &&
			countMatches(item.mode, uint64(metadata.Mode&0o777)) && countMatches(item.uid, uint64(metadata.UID)) && countMatches(item.gid, uint64(metadata.GID))
		if unchanged {
			descriptor, failed := securityOf(selected, false)
			if failed != nil {
				return failed
			}
			unchanged = string(descriptor) == string(item.security)
		}
		if !unchanged {
			return revisionChanged(selected)
		}
		if item.hasData {
			data, failed := readFile(selected, metadata, maxResourceFileBytes, nil)
			if failed != nil {
				return failed
			}
			if string(data) != string(item.data) {
				return failure(CodeConcurrentChange, "file changed while reading", selected)
			}
			continue
		}
		if childrenByParent == nil {
			childrenByParent = observedChildren(observed.nodes)
		}
		expected := childrenByParent[item.path]
		children, failed := directoryChildren(selected, metadata, len(expected))
		if failed != nil {
			return failed
		}
		if !slices.Equal(childNames(children), expected) {
			return failure(CodeConcurrentChange, "directory entries changed", selected)
		}
	}
	return nil
}

func observedChildren(nodes []node) map[string][]string {
	children := map[string][]string{}
	for _, candidate := range nodes {
		if candidate.path == "" {
			continue
		}
		parent, name := "", candidate.path
		if index := strings.LastIndexByte(candidate.path, '/'); index >= 0 {
			parent, name = candidate.path[:index], candidate.path[index+1:]
		}
		if name != "" {
			children[parent] = append(children[parent], name)
		}
	}
	for _, names := range children {
		slices.Sort(names)
	}
	return children
}

func optionalLstat(path string) (*sysx.Metadata, *TargetError) {
	metadata, err := sysx.LstatMetadata(path)
	switch {
	case notFound(err):
		return nil, nil
	case err != nil:
		return nil, ioFailure(err, path)
	}
	return &metadata, nil
}

func sameOptionalMetadata(a, b *sysx.Metadata) bool {
	if a == nil || b == nil {
		return a == b
	}
	return *a == *b
}

func observe(path string) (revision, *TargetError) {
	parents, failed := captureAncestors(path)
	if failed != nil {
		return revision{}, failed
	}
	initial, failed := optionalLstat(path)
	if failed != nil {
		return revision{}, failed
	}
	walk := observation{signatures: map[string]sysx.Metadata{}}
	if failed := walk.visit(path, "", 0); failed != nil {
		return revision{}, failed
	}
	final, failed := optionalLstat(path)
	if failed != nil {
		return revision{}, failed
	}
	if !sameOptionalMetadata(initial, final) {
		return revision{}, revisionChanged(path)
	}
	current, failed := captureAncestors(path)
	if failed != nil {
		return revision{}, failed
	}
	if !ancestorsEqual(current, parents) {
		return revision{}, failure(CodeConcurrentChange, "resource ancestor changed", path)
	}
	slices.SortFunc(walk.nodes, func(a, b node) int { return strings.Compare(a.path, b.path) })
	observed, err := newRevision(walk.nodes)
	if err != nil {
		return revision{}, failure(CodeInvalidState, err.Error(), path)
	}
	if failed := verifyObserved(path, observed, walk.signatures); failed != nil {
		return revision{}, failed
	}
	return observed, nil
}

func require(path string, expected revision) *TargetError {
	actual, failed := observe(path)
	if failed != nil {
		return failed
	}
	if !actual.equal(expected) {
		return failure(CodeConcurrentChange, "complete resource revision changed", path)
	}
	return nil
}

func requireUnchangedSince(path string, expected revision, boundary time.Time) *TargetError {
	for _, item := range expected.nodes {
		selected := nodePath(path, item)
		_, err := sysx.LstatMetadata(selected)
		switch {
		case notFound(err):
			return revisionChanged(selected)
		case err != nil:
			return ioFailure(err, selected)
		}
		times, err := native.FileTimes(selected)
		if err != nil {
			return nativeFailure(err)
		}
		if times.ChangedAfter(boundary) {
			return failure(CodeRecoveryNeeded, "destination revision changed after preparation", selected)
		}
	}
	return nil
}

func checkRevisionBudget(planned revision, path string) *TargetError {
	if len(planned.nodes) > maxResourceNodes {
		return limitFailure("resource exceeds the node limit", path)
	}
	var total int64
	for _, item := range planned.nodes {
		if item.path != "" && strings.Count(item.path, "/")+1 > maxResourceDepth {
			return limitFailure("resource exceeds the node limit", path)
		}
		total += int64(len(item.data))
		if len(item.data) > maxResourceFileBytes || total > maxResourceTotalBytes {
			return limitFailure("resource exceeds the byte limit", path)
		}
	}
	return nil
}

func countU32(c count) (uint32, bool) {
	value, ok := c.uint64()
	if !ok || value > math.MaxUint32 {
		return 0, false
	}
	return uint32(value), true
}

func createNode(parent int, name, destination string, item node) (int, *TargetError) {
	if item.hasData {
		fd, err := sysx.CreateExclusiveAt(parent, name, destination)
		if err != nil {
			return -1, ioFailure(err, destination)
		}
		if err := sysx.WriteAll(fd, destination, item.data); err != nil {
			_ = sysx.Close(fd)
			return -1, ioFailure(err, "")
		}
		return fd, nil
	}
	if err := sysx.MkdirAt(parent, name, destination, 0o700); err != nil {
		return -1, ioFailure(err, destination)
	}
	fd, err := sysx.OpenDirectoryAt(parent, name, destination)
	if err != nil {
		return -1, ioFailure(err, destination)
	}
	return fd, nil
}

func nodeMetadata(destination string, item node) (mode, uid, gid uint32, failed *TargetError) {
	mode, okMode := countU32(item.mode)
	uid, okUID := countU32(item.uid)
	gid, okGID := countU32(item.gid)
	if !okMode || !okUID || !okGID {
		return 0, 0, 0, failure(CodeInvalidState, "invalid revision metadata", destination)
	}
	return mode, uid, gid, nil
}

func applyNodeMetadata(fd int, destination string, item node) *TargetError {
	mode, uid, gid, failed := nodeMetadata(destination, item)
	if failed != nil {
		return failed
	}
	metadata, err := sysx.FstatMetadata(fd, destination)
	if err != nil {
		return ioFailure(err, destination)
	}
	if metadata.UID != uid || metadata.GID != gid {
		if err := sysx.Fchown(fd, destination, uid, gid); err != nil {
			return ioFailure(err, destination)
		}
	}
	if err := sysx.Fchmod(fd, destination, mode); err != nil {
		return ioFailure(err, destination)
	}
	return nil
}

func openStagedDirectory(root int, path, relative string) (int, *TargetError) {
	current, walked := root, path
	for _, name := range strings.Split(relative, "/") {
		walked = pathx.Join(walked, name)
		next, err := sysx.OpenDirectoryAt(current, name, walked)
		if current != root {
			_ = sysx.Close(current)
		}
		if err != nil {
			return -1, ioFailure(err, walked)
		}
		current = next
	}
	return current, nil
}

func closeStaged(fd int, destination string, failed *TargetError) *TargetError {
	if err := sysx.Close(fd); err != nil && failed == nil {
		return ioFailure(err, destination)
	}
	return failed
}

func stageChild(root int, path string, item node) *TargetError {
	destination := nodePath(path, item)
	parent, name := root, item.path
	if cut := strings.LastIndexByte(item.path, '/'); cut >= 0 {
		var failed *TargetError
		if parent, failed = openStagedDirectory(root, path, item.path[:cut]); failed != nil {
			return failed
		}
		name = item.path[cut+1:]
		defer func() { _ = sysx.Close(parent) }()
	}
	fd, failed := createNode(parent, name, destination, item)
	if failed != nil {
		return failed
	}
	return closeStaged(fd, destination, applyNodeMetadata(fd, destination, item))
}

func stageRevision(path string, planned revision) *TargetError {
	if _, err := newRevision(planned.nodes); err != nil {
		return failure(CodeInvalidState, err.Error(), path)
	}
	top := planned.nodes[0]
	root, failed := createNode(sysx.WorkingDirectory, path, path, top)
	if failed != nil {
		return failed
	}
	if failed := applyNodeMetadata(root, path, top); failed != nil || top.hasData {
		return closeStaged(root, path, failed)
	}
	for _, item := range planned.nodes[1:] {
		if failed := stageChild(root, path, item); failed != nil {
			return closeStaged(root, path, failed)
		}
	}
	return closeStaged(root, path, nil)
}

type intendedNode struct {
	path    string
	data    string
	hasData bool
	mode    count
	uid     count
	gid     count
}

func intendedNodes(r revision) []intendedNode {
	result := make([]intendedNode, 0, len(r.nodes))
	for _, item := range r.nodes {
		result = append(result, intendedNode{path: item.path, data: string(item.data), hasData: item.hasData, mode: item.mode, uid: item.uid, gid: item.gid})
	}
	return result
}

func writeRevision(path string, planned revision) (revision, *TargetError) {
	if failed := checkRevisionBudget(planned, path); failed != nil {
		return revision{}, failed
	}
	if len(planned.nodes) > 0 {
		if failed := stageRevision(path, planned); failed != nil {
			return revision{}, failed
		}
	}
	result, failed := observe(path)
	if failed != nil {
		return revision{}, failed
	}
	if !slices.Equal(intendedNodes(planned), intendedNodes(result)) {
		return revision{}, failure(CodeConcurrentChange, "staged resource differs from intended revision", path)
	}
	return result, nil
}

func cleanupRevision(current, expected revision) bool {
	return len(current.nodes) <= len(expected.nodes) && slices.EqualFunc(current.nodes, expected.nodes[:len(current.nodes)], node.equal)
}

// An absent resource is already destroyed.
func destroy(ctx context.Context, path string, expected revision) *TargetError {
	actual, failed := observe(path)
	if failed != nil {
		return failed
	}
	if len(actual.nodes) == 0 {
		return nil
	}
	if !actual.equal(expected) {
		return failure(CodeRecoveryNeeded, "cleanup revision changed", path)
	}
	for _, item := range slices.Backward(expected.nodes) {
		selected := nodePath(path, item)
		if !item.hasData {
			if err := sysx.Rmdir(selected); err != nil {
				return ioFailure(err, selected)
			}
			continue
		}
		seams.Fire(ctx, seams.Event{Kind: seams.KindUnlink, Path: selected})
		if err := sysx.Unlink(selected); err != nil {
			return ioFailure(err, selected)
		}
	}
	return nil
}

func renameExclusive(ctx context.Context, source, destination string) *TargetError {
	seams.Fire(ctx, seams.Event{Kind: seams.KindRename, Source: source, Destination: destination})
	if err := sysx.RenameExclusive(source, destination); err != nil {
		errno := sysx.Errno(err)
		message := err.Error()
		if errno != 0 {
			message = sysx.PosixPathErrorMessage(errno, destination)
		}
		return newTargetError(ioCode(errno), message, destination, errno, err)
	}
	seams.Fire(ctx, seams.Event{Kind: seams.KindRename, Source: source, Destination: destination, Done: true})
	return nil
}

func moveRevision(ctx context.Context, source, destination string, expected revision) *TargetError {
	if failed := require(source, expected); failed != nil {
		return failed
	}
	if failed := renameExclusive(ctx, source, destination); failed != nil {
		return failed
	}
	return require(destination, expected)
}

func validateKind(reference ResourceRef) *TargetError {
	destination := reference.Destination
	metadata, err := sysx.LstatMetadata(destination)
	switch {
	case notFound(err):
		return nil
	case err != nil:
		return ioFailure(err, destination)
	}
	if failed := managedMetadata(metadata, destination); failed != nil {
		return failed
	}
	if metadata.IsRegular() != (reference.Kind == ResourceDocument) {
		return failure(CodeUnsafePath, "resource kind disagrees with existing destination", destination)
	}
	return nil
}

func createParents(path string, created []ancestor) ([]ancestor, *TargetError) {
	result := slices.Clone(created)
	for _, parent := range parentPaths(path) {
		if _, err := sysx.StatMetadata(parent); err == nil {
			continue
		}
		err := sysx.Mkdir(parent, 0o700)
		switch {
		case errors.Is(err, syscall.EEXIST):
			continue
		case err != nil:
			return nil, ioFailure(err, parent)
		}
		captured, failed := captureAncestors(pathx.Join(parent, "child"))
		if failed != nil {
			return nil, failed
		}
		if index := slices.IndexFunc(captured, func(item ancestor) bool { return item.path == parent }); index >= 0 {
			result = append(result, captured[index])
		}
	}
	return result, nil
}

func createStoreParents(st store.Store, created []ancestor) ([]ancestor, *TargetError) {
	result := created
	if location, ok := st.Location(); ok {
		physical, err := native.PhysicalLocation(location)
		if err != nil {
			return nil, errorDetail(nativeAuthorityError(err))
		}
		var failed *TargetError
		if result, failed = createParents(physical, result); failed != nil {
			return nil, failed
		}
	}
	if staging, ok := st.StagingDirectory(); ok {
		physical, err := native.PhysicalLocation(staging)
		if err != nil {
			return nil, errorDetail(nativeAuthorityError(err))
		}
		var failed *TargetError
		if result, failed = createParents(pathx.Join(physical, "transaction"), result); failed != nil {
			return nil, failed
		}
	}
	return slices.Clone(result), nil
}

func stagedTransactionDirectory(staging, destination, transactionID string) (string, bool) {
	root, err := native.PhysicalLocation(staging)
	if err != nil {
		return "", false
	}
	return pathx.Join(root, destinationKey(destination)+"-"+transactionID), true
}

func chooseTransactionDirectory(st store.Store, destination, transactionID string) (string, *TargetError) {
	fallback, err := TransactionPath(destination, transactionID)
	if err != nil {
		return "", newTargetError(CodeInvalidState, err.Error(), "", 0, err)
	}
	staging, ok := st.StagingDirectory()
	if !ok {
		return fallback, nil
	}
	selected, ok := stagedTransactionDirectory(staging, destination, transactionID)
	if !ok {
		return fallback, nil
	}
	left, hasLeft := pathx.Parent(selected)
	right, hasRight := pathx.Parent(destination)
	if !hasLeft || !hasRight {
		return fallback, nil
	}
	if shared, err := native.SameMount(left, right); err != nil || !shared {
		return fallback, nil
	}
	return selected, nil
}

func recognizedTransactionDirectory(st store.Store, destination, transactionID, directory string) bool {
	if fallback, err := TransactionPath(destination, transactionID); err == nil && pathsEqual(fallback, directory) {
		return true
	}
	staging, ok := st.StagingDirectory()
	if !ok {
		return false
	}
	selected, ok := stagedTransactionDirectory(staging, destination, transactionID)
	return ok && pathsEqual(selected, directory)
}
