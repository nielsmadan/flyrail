//go:build darwin || linux

package flyrail

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
	"syscall"
	"testing"
	"time"

	"github.com/google/go-cmp/cmp"

	"github.com/nielsmadan/flyrail/go/flyrail/store"
	"github.com/nielsmadan/flyrail/go/internal/seams"
	"github.com/nielsmadan/flyrail/go/internal/sysx"
)

func documentRef(t *testing.T, destination string) ResourceRef {
	t.Helper()
	return ResourceRef{Destination: authority(t, destination).Destination(), Kind: ResourceDocument}
}

func treeRef(t *testing.T, destination string) ResourceRef {
	t.Helper()
	return ResourceRef{Destination: authority(t, destination).Destination(), Kind: ResourceTree}
}

func authorityKey(reference ResourceRef) string {
	return resourceKey(reference.Destination, "authority")
}

func wantFailure(t *testing.T, failed *TargetError, code ErrorCode, message, path string) {
	t.Helper()
	if failed == nil {
		t.Fatalf("failure = nil, want %s %q", code, message)
	}
	if failed.Code != code || failed.Message != message || failed.Path != path {
		t.Fatalf("failure = %s %q at %q, want %s %q at %q", failed.Code, failed.Message, failed.Path, code, message, path)
	}
}

func mustSucceed(t *testing.T, failed *TargetError) {
	t.Helper()
	if failed != nil {
		t.Fatalf("unexpected failure: %s %q at %q", failed.Code, failed.Message, failed.Path)
	}
}

func mustObserve(t *testing.T, path string) revision {
	t.Helper()
	observed, failed := observe(path)
	mustSucceed(t, failed)
	return observed
}

func storeValue(t *testing.T, st store.Store, key string) []byte {
	t.Helper()
	value, ok, err := st.Read(t.Context(), key)
	if err != nil || !ok {
		t.Fatalf("Read(%q) = %v, %v", key, ok, err)
	}
	return value
}

func caseInsensitive(t *testing.T, root string) bool {
	t.Helper()
	probe := filepath.Join(root, "case-probe")
	if err := os.Mkdir(probe, 0o700); err != nil {
		t.Fatal(err)
	}
	defer func() { _ = os.Remove(probe) }()
	_, err := os.Lstat(filepath.Join(root, "CASE-PROBE"))
	return err == nil
}

func TestAdmissionWritesAndReplaysTheAuthorityRecord(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	st := store.NewMemoryStore()
	reference := documentRef(t, filepath.Join(root, "AGENTS.md"))
	mustSucceed(t, admit(t.Context(), st, reference))
	recorded := storeValue(t, st, authorityKey(reference))
	if string(recorded) != string(encodeRecord(reference)) {
		t.Fatalf("authority record = %q", recorded)
	}
	mustSucceed(t, admit(t.Context(), st, reference))
	if again := storeValue(t, st, authorityKey(reference)); string(again) != string(recorded) {
		t.Fatalf("re-admitted authority record = %q", again)
	}
	if names := directoryNames(t, root); len(names) != 0 {
		t.Fatalf("admission created %v", names)
	}
}

func TestAdmissionRefusesDisagreeingAuthorityIdentity(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	st := store.NewMemoryStore()
	reference := documentRef(t, filepath.Join(root, "AGENTS.md"))
	other := ResourceRef{Destination: reference.Destination, Kind: ResourceTree}
	if ok, err := st.Write(t.Context(), authorityKey(reference), encodeRecord(other), store.Absent()); !ok || err != nil {
		t.Fatal(ok, err)
	}
	wantFailure(t, admit(t.Context(), st, reference), CodeInvalidState, "authority identity disagrees", reference.Destination)
}

func TestAdmissionRefusesKindDisagreement(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	st := store.NewMemoryStore()
	document := filepath.Join(root, "AGENTS.md")
	writeFile(t, document, "document")
	wantFailure(t, admit(t.Context(), st, treeRef(t, document)), CodeUnsafePath, "resource kind disagrees with existing destination", document)
	directory := filepath.Join(root, "tree")
	if err := os.Mkdir(directory, 0o700); err != nil {
		t.Fatal(err)
	}
	wantFailure(t, admit(t.Context(), st, documentRef(t, directory)), CodeUnsafePath, "resource kind disagrees with existing destination", directory)
}

func TestAdmissionRefusesAncestorAndDescendantResources(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	st := store.NewMemoryStore()
	treeDestination := filepath.Join(root, "config")
	if err := os.Mkdir(treeDestination, 0o700); err != nil {
		t.Fatal(err)
	}
	tree := treeRef(t, treeDestination)
	mustSucceed(t, admit(t.Context(), st, tree))
	child := documentRef(t, filepath.Join(treeDestination, "child.md"))
	wantFailure(t, admit(t.Context(), st, child), CodeConflict, "ancestor or descendant resource conflicts", tree.Destination)
	wantFailure(t, admit(t.Context(), st, tree), CodeConflict, "ancestor or descendant resource conflicts", child.Destination)
	if got := storeValue(t, st, authorityKey(child)); string(got) != string(encodeRecord(child)) {
		t.Fatalf("refused child kept authority %q", got)
	}
}

func TestAdmissionTreatsTransactionDirectoriesInsideATreeAsFences(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	st := store.NewMemoryStore()
	destination := filepath.Join(root, "skills")
	retained := filepath.Join(destination, "nested", ".flyrail-"+strings.Repeat("a", 64)+"-"+testTransaction+".txn")
	if err := os.MkdirAll(retained, 0o700); err != nil {
		t.Fatal(err)
	}
	wantFailure(t, admit(t.Context(), st, treeRef(t, destination)), CodeConflict, "ancestor or descendant resource conflicts", retained)
}

func TestAdmissionRejectsAChangedCanonicalDestination(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	if !caseInsensitive(t, root) {
		t.Skip("host is case sensitive")
	}
	if err := os.Mkdir(filepath.Join(root, "Config"), 0o700); err != nil {
		t.Fatal(err)
	}
	recorded := ResourceRef{Destination: filepath.Join(root, "config", "AGENTS.md"), Kind: ResourceDocument}
	wantFailure(t, admit(t.Context(), store.NewMemoryStore(), recorded), CodeInvalidState, "resource canonical destination changed", "")
}

func TestAdmissionRefusesWrongTypedAndNoncanonicalAuthorityRecords(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	st := store.NewMemoryStore()
	reference := documentRef(t, filepath.Join(root, "AGENTS.md"))
	key := authorityKey(reference)
	if ok, err := st.Write(t.Context(), key, []byte("{\"enum\": \"Ownership\", \"value\": \"file\"}\n"), store.Absent()); !ok || err != nil {
		t.Fatal(ok, err)
	}
	wantFailure(t, admit(t.Context(), st, reference), CodeInvalidState, key+": record has the wrong type", "")
	other := documentRef(t, filepath.Join(root, "OTHER.md"))
	foreign := authorityKey(other)
	encoded := encodeRecord(other)
	spaced := append([]byte{encoded[0], ' '}, encoded[1:]...)
	if ok, err := st.Write(t.Context(), foreign, spaced, store.Absent()); !ok || err != nil {
		t.Fatal(ok, err)
	}
	wantFailure(t, admit(t.Context(), st, other), CodeInvalidState, foreign+": record is not canonical", "")
}

func TestAdmissionRefusesAnInvalidReference(t *testing.T) {
	t.Parallel()
	wantFailure(t, admit(t.Context(), store.NewMemoryStore(), ResourceRef{Destination: "relative", Kind: ResourceDocument}),
		CodeInvalidState, "expected an absolute concrete path", "relative")
}

func TestAdmissionHonoursCancellationBeforeWriting(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	st := store.NewMemoryStore()
	reference := documentRef(t, filepath.Join(root, "AGENTS.md"))
	ctx, cancel := context.WithCancel(t.Context())
	cancel()
	failed := admit(ctx, st, reference)
	if failed == nil || !errors.Is(failed, context.Canceled) {
		t.Fatalf("admit after cancellation = %v, want context.Canceled", failed)
	}
	keys, err := st.Keys(t.Context(), "resource/")
	if err != nil || len(keys) != 0 {
		t.Fatalf("cancelled admission wrote %v (%v)", keys, err)
	}
}

func TestRecordsRoundTripWithCompareAndSet(t *testing.T) {
	t.Parallel()
	st := store.NewMemoryStore()
	key := indexKey("team", strings.Repeat("a", 64))
	recorded := must(newIndex("team", strings.Repeat("a", 64), nil, nil, nil, nil, stateSchemaVersion))
	if _, ok, failed := readRecord(t.Context(), st, key, "Index", indexFromState); ok || failed != nil {
		t.Fatalf("readRecord of absent = %v, %v", ok, failed)
	}
	mustSucceed(t, writeRecord(t.Context(), st, key, recorded, nil))
	read, ok, failed := readRecord(t.Context(), st, key, "Index", indexFromState)
	mustSucceed(t, failed)
	if !ok || string(encodeRecord(read)) != string(encodeRecord(recorded)) {
		t.Fatalf("readRecord = %v", ok)
	}
	wantFailure(t, writeRecord(t.Context(), st, key, recorded, nil), CodeConcurrentChange, key+": state changed", "")
	mustSucceed(t, writeRecord(t.Context(), st, key, recorded, &recorded))
	mustSucceed(t, deleteRecord(t.Context(), st, key, recorded))
	wantFailure(t, deleteRecord(t.Context(), st, key, recorded), CodeConcurrentChange, key+": state changed", "")
}

func TestRecordsTreatANilExpectationAsAbsent(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	st := store.NewMemoryStore()
	reference := documentRef(t, filepath.Join(root, "AGENTS.md"))
	key := receiptKey(reference)
	recorded := must(newReceipt(reference, testTransaction, nil, DocumentProvenance{}, nil, stateSchemaVersion))
	var previous *receipt
	mustSucceed(t, writeRecord(t.Context(), st, key, recorded, previous))
	if diff := cmp.Diff(string(encodeRecord(recorded)), string(storeValue(t, st, key))); diff != "" {
		t.Errorf("first write (-want +got):\n%s", diff)
	}
	wantFailure(t, writeRecord(t.Context(), st, key, recorded, previous), CodeConcurrentChange, key+": state changed", "")
	previous = &recorded
	mustSucceed(t, writeRecord(t.Context(), st, key, recorded, previous))

	absent := receiptKey(documentRef(t, filepath.Join(root, "OTHER.md")))
	wantFailure(t, deleteRecord(t.Context(), st, absent, recorded), CodeConcurrentChange, absent+": state changed", "")
	keys, err := st.Keys(t.Context(), "resource/")
	if err != nil {
		t.Fatal(err)
	}
	if diff := cmp.Diff([]string{key}, keys); diff != "" {
		t.Errorf("stored keys after deleting an absent record (-want +got):\n%s", diff)
	}
}

func TestStoreLockReportsABusyLockAtTheStoreLocation(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	st, err := store.NewDirectoryStore(t.Context(), filepath.Join(root, "state"))
	if err != nil {
		t.Fatal(err)
	}
	held, failed := storeLock(t.Context(), st, "index/team/x", 0)
	mustSucceed(t, failed)
	defer func() { _ = held.Unlock() }()
	_, failed = storeLock(t.Context(), st, "index/team/x", 0)
	wantFailure(t, failed, CodeBusy, "state store lock is busy: index/team/x", filepath.Join(root, "state"))
	if !errors.Is(failed, store.ErrBusy) {
		t.Errorf("busy failure does not wrap store.ErrBusy")
	}
}

func TestObservationRejectsFileAncestors(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	file := filepath.Join(root, "plainfile")
	writeFile(t, file, "file")
	_, failed := observe(filepath.Join(file, "doc.md"))
	wantFailure(t, failed, CodeUnsafePath, "resource ancestor is not an ordinary directory", file)
}

func TestObservationCapturesPhysicalIdentityAndContent(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	path := filepath.Join(root, "skills")
	writeFile(t, filepath.Join(path, "b", "SKILL.md"), "# B\n")
	writeFile(t, filepath.Join(path, "a.md"), "a")
	if err := os.Chmod(filepath.Join(path, "a.md"), 0o640); err != nil {
		t.Fatal(err)
	}
	observed := mustObserve(t, path)
	type summary struct {
		Path string
		Data string
		File bool
		Mode int64
	}
	var got []summary
	for _, item := range observed.nodes {
		got = append(got, summary{Path: item.path, Data: string(item.data), File: item.hasData, Mode: item.mode.small})
		info, err := os.Lstat(nodePath(path, item))
		if err != nil {
			t.Fatal(err)
		}
		stat := info.Sys().(*syscall.Stat_t)
		if !countMatches(item.inode, stat.Ino) || item.uid.small != int64(stat.Uid) {
			t.Errorf("node %q identity = %+v/%+v, want inode %d uid %d", item.path, item.inode, item.uid, stat.Ino, stat.Uid)
		}
	}
	want := []summary{
		{Path: "", Mode: 0o700},
		{Path: "a.md", Data: "a", File: true, Mode: 0o640},
		{Path: "b", Mode: 0o700},
		{Path: "b/SKILL.md", Data: "# B\n", File: true, Mode: 0o600},
	}
	if diff := cmp.Diff(want, got); diff != "" {
		t.Errorf("observed nodes (-want +got):\n%s", diff)
	}
	if absent := mustObserve(t, filepath.Join(root, "missing")); len(absent.nodes) != 0 {
		t.Errorf("absent observation = %d nodes", len(absent.nodes))
	}
}

func TestSnapshotsDetectSameLengthRewritesAndMembershipChanges(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	path := filepath.Join(root, "AGENTS.md")
	writeFile(t, path, "before")
	expected := mustObserve(t, path)
	mustSucceed(t, require(path, expected))
	if err := os.WriteFile(path, []byte("change"), 0o600); err != nil {
		t.Fatal(err)
	}
	wantFailure(t, require(path, expected), CodeConcurrentChange, "complete resource revision changed", path)
	directory := filepath.Join(root, "skills")
	writeFile(t, filepath.Join(directory, "one"), "one")
	expected = mustObserve(t, directory)
	writeFile(t, filepath.Join(directory, "two"), "two")
	wantFailure(t, require(directory, expected), CodeConcurrentChange, "complete resource revision changed", directory)
}

func TestAncestorSnapshotsDetectParentRebinding(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	parent := filepath.Join(root, "parent")
	path := filepath.Join(parent, "AGENTS.md")
	writeFile(t, path, "content")
	expected, failed := captureAncestors(path)
	mustSucceed(t, failed)
	if err := os.Rename(parent, filepath.Join(root, "replaced")); err != nil {
		t.Fatal(err)
	}
	writeFile(t, path, "content")
	rebound, failed := captureAncestors(path)
	mustSucceed(t, failed)
	if ancestorsEqual(rebound, expected) {
		t.Fatal("rebound ancestors compare equal")
	}
	if rebound[len(rebound)-1].path != parent {
		t.Errorf("last ancestor = %q, want %q", rebound[len(rebound)-1].path, parent)
	}
}

func TestObservationRefusesLinksSpecialFilesAndUnsafeModes(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	target := filepath.Join(root, "target.md")
	writeFile(t, target, "target")
	link := filepath.Join(root, "link.md")
	if err := os.Symlink(target, link); err != nil {
		t.Fatal(err)
	}
	const unsupported = "managed symlinks, reparse points and special files are unsupported"
	_, failed := observe(link)
	wantFailure(t, failed, CodeUnsafePath, unsupported, link)
	hardlink := filepath.Join(root, "hard.md")
	if err := os.Link(target, hardlink); err != nil {
		t.Fatal(err)
	}
	_, failed = observe(hardlink)
	wantFailure(t, failed, CodeUnsafePath, unsupported, hardlink)
	tree := filepath.Join(root, "tree")
	writeFile(t, filepath.Join(tree, "ok.md"), "ok")
	inner := filepath.Join(tree, "escape")
	if err := os.Symlink(root, inner); err != nil {
		t.Fatal(err)
	}
	_, failed = observe(tree)
	wantFailure(t, failed, CodeUnsafePath, unsupported, inner)
	fifo := filepath.Join(root, "fifo")
	if err := syscall.Mkfifo(fifo, 0o600); err != nil {
		t.Fatal(err)
	}
	_, failed = observe(fifo)
	wantFailure(t, failed, CodeUnsafePath, unsupported, fifo)
	sticky := filepath.Join(root, "sticky")
	if err := os.Mkdir(sticky, 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.Chmod(sticky, 0o700|os.ModeSticky); err != nil {
		t.Fatal(err)
	}
	_, failed = observe(sticky)
	wantFailure(t, failed, CodeUnsafePath, "special permission bits are unsupported", sticky)
	linkedParent := filepath.Join(root, "linked-parent")
	if err := os.Symlink(tree, linkedParent); err != nil {
		t.Fatal(err)
	}
	_, failed = observe(filepath.Join(linkedParent, "ok.md"))
	wantFailure(t, failed, CodeUnsafePath, unsupported, linkedParent)
}

func TestObservationRefusesNonportableAndCollidingNames(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	tree := filepath.Join(root, "tree")
	if err := os.Mkdir(tree, 0o700); err != nil {
		t.Fatal(err)
	}
	invalid := filepath.Join(tree, "bad\xff")
	if err := os.WriteFile(invalid, []byte("x"), 0o600); err != nil {
		t.Skipf("filesystem refuses non-UTF-8 names: %v", err)
	}
	_, failed := observe(tree)
	wantFailure(t, failed, CodeUnsafePath, "nonportable resource path", invalid)
	if err := os.Remove(invalid); err != nil {
		t.Fatal(err)
	}
	if caseInsensitive(t, root) {
		return
	}
	writeFile(t, filepath.Join(tree, "Readme.md"), "a")
	writeFile(t, filepath.Join(tree, "README.md"), "b")
	_, failed = observe(tree)
	wantFailure(t, failed, CodeUnsafePath, "portable filename collision", filepath.Join(tree, "Readme.md"))
}

func TestObservationEnforcesTheResourceBudgets(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	large := filepath.Join(root, "large.md")
	file, err := os.Create(large)
	if err != nil {
		t.Fatal(err)
	}
	if err := file.Truncate(maxResourceFileBytes + 1); err != nil {
		t.Fatal(err)
	}
	if err := file.Close(); err != nil {
		t.Fatal(err)
	}
	_, failed := observe(large)
	wantFailure(t, failed, CodeUnsupported, "[Errno "+strconv.Itoa(int(syscall.ENOTSUP))+"] resource exceeds the byte limit: PosixPath('"+large+"')", large)
	if !errors.Is(failed, syscall.ENOTSUP) {
		t.Errorf("byte limit failure does not wrap ENOTSUP")
	}
	deep := filepath.Join(root, "deep")
	path := deep
	for range maxResourceDepth + 1 {
		path = filepath.Join(path, "d")
	}
	if err := os.MkdirAll(path, 0o700); err != nil {
		t.Fatal(err)
	}
	_, failed = observe(deep)
	wantFailure(t, failed, CodeUnsupported, "[Errno "+strconv.Itoa(int(syscall.ENOTSUP))+"] resource exceeds the node limit: PosixPath('"+path+"')", path)
	nodes := make([]node, 0, maxResourceNodes+1)
	for index := range maxResourceNodes + 1 {
		nodes = append(nodes, node{path: strings.Repeat("x", index)})
	}
	wantFailure(t, checkRevisionBudget(revision{nodes: nodes}, deep), CodeUnsupported,
		"[Errno "+strconv.Itoa(int(syscall.ENOTSUP))+"] resource exceeds the node limit: PosixPath('"+deep+"')", deep)
	big := revision{nodes: []node{{hasData: true, data: make([]byte, maxResourceFileBytes+1)}}}
	wantFailure(t, checkRevisionBudget(big, deep), CodeUnsupported,
		"[Errno "+strconv.Itoa(int(syscall.ENOTSUP))+"] resource exceeds the byte limit: PosixPath('"+deep+"')", deep)
	mustSucceed(t, checkRevisionBudget(revision{nodes: []node{{hasData: true, data: make([]byte, 16)}}}, deep))
}

func TestBirthTimeIsComparedOnlyWhereRecorded(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	path := filepath.Join(root, "AGENTS.md")
	writeFile(t, path, "# mine\n")
	earlier := time.Now().Add(-2 * time.Minute)
	if err := os.Chtimes(path, earlier, earlier); err != nil {
		t.Fatal(err)
	}
	observed := mustObserve(t, path)
	mustSucceed(t, requireUnchangedSince(path, observed, earlier.Add(time.Minute*3)))
	failed := requireUnchangedSince(path, observed, earlier.Add(-time.Minute))
	wantFailure(t, failed, CodeRecoveryNeeded, "destination revision changed after preparation", path)
	if err := os.Remove(path); err != nil {
		t.Fatal(err)
	}
	wantFailure(t, requireUnchangedSince(path, observed, earlier), CodeConcurrentChange, "path changed during inspection", path)
}

func plannedTree(t *testing.T, entries ...BundleEntry) revision {
	t.Helper()
	return must(desiredRevision(must(NewTreeContent(entries))))
}

func TestWriteRevisionStagesExclusivelyWithRecordedMetadata(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	planned := plannedTree(t,
		BundleEntry{Path: "SKILL.md", Data: []byte("# Skill\n")},
		BundleEntry{Path: "bin/run", Data: []byte("#!/bin/sh\n"), Executable: true},
	)
	path := filepath.Join(root, "staging")
	written, failed := writeRevision(path, planned)
	mustSucceed(t, failed)
	if !intact(written, planned) {
		t.Fatal("written revision is not intact")
	}
	modes := map[string]os.FileMode{}
	for _, item := range written.nodes {
		info, err := os.Lstat(nodePath(path, item))
		if err != nil {
			t.Fatal(err)
		}
		modes[item.path] = info.Mode().Perm()
	}
	want := map[string]os.FileMode{"": 0o755, "SKILL.md": 0o644, "bin": 0o755, "bin/run": 0o755}
	if diff := cmp.Diff(want, modes); diff != "" {
		t.Errorf("staged modes (-want +got):\n%s", diff)
	}
	_, failed = writeRevision(path, planned)
	if failed == nil || failed.Errno != syscall.EEXIST || failed.Path != path {
		t.Fatalf("second writeRevision = %v, want EEXIST at %q", failed, path)
	}
	document := filepath.Join(root, "link.md")
	if err := os.Symlink(filepath.Join(root, "elsewhere"), document); err != nil {
		t.Fatal(err)
	}
	file := must(desiredRevision(must(NewFileContent([]byte("x"), ModePrivate))))
	_, failed = writeRevision(document, file)
	if failed == nil || failed.Errno != syscall.EEXIST {
		t.Fatalf("writeRevision over a symlink = %v, want EEXIST", failed)
	}
	if _, err := os.Lstat(filepath.Join(root, "elsewhere")); !errors.Is(err, os.ErrNotExist) {
		t.Fatalf("writeRevision followed the link: %v", err)
	}
}

func plannedNode(t *testing.T, path string, data string, hasData bool, mode int64) node {
	t.Helper()
	return must(newPlannedNode(path, []byte(data), hasData, mode))
}

func TestWriteRevisionRefusesChildrenOfReadOnlyDirectoriesAsRust(t *testing.T) {
	t.Parallel()
	if os.Geteuid() == 0 {
		t.Skip("root ignores directory permissions")
	}
	cases := []struct {
		name    string
		nodes   []node
		failing string
		left    int
	}{
		{
			name: "subdirectory",
			nodes: []node{
				plannedNode(t, "", "", false, 0o755),
				plannedNode(t, "bin", "", false, 0o555),
				plannedNode(t, "bin/run", "#!/bin/sh\n", true, 0o755),
				plannedNode(t, "notes.md", "notes", true, 0o644),
			},
			failing: "bin/run",
			left:    2,
		},
		{
			name: "root",
			nodes: []node{
				plannedNode(t, "", "", false, 0o500),
				plannedNode(t, "bin", "", false, 0o755),
			},
			failing: "bin",
			left:    1,
		},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			t.Parallel()
			path := filepath.Join(tempRoot(t), "staging")
			t.Cleanup(func() {
				_ = filepath.WalkDir(path, func(walked string, entry os.DirEntry, err error) error {
					if err == nil && entry.IsDir() {
						_ = os.Chmod(walked, 0o700)
					}
					return nil
				})
			})
			planned := must(newRevision(tc.nodes))
			_, failed := writeRevision(path, planned)
			failing := filepath.Join(path, tc.failing)
			wantFailure(t, failed, CodeIOError, "[Errno 13] Permission denied: '"+failing+"'", failing)
			if failed.Errno != syscall.EACCES {
				t.Errorf("errno = %v, want EACCES", failed.Errno)
			}
			left := mustObserve(t, path)
			if diff := cmp.Diff(intendedNodes(planned)[:tc.left], intendedNodes(left), cmp.AllowUnexported(intendedNode{}, count{})); diff != "" {
				t.Errorf("partial staging is not the planned prefix (-want +got):\n%s", diff)
			}
			mustSucceed(t, destroy(t.Context(), path, left))
			if _, err := os.Lstat(path); !errors.Is(err, os.ErrNotExist) {
				t.Errorf("destroy left %q behind: %v", path, err)
			}
		})
	}
}

func TestStagingNeverFollowsASwappedIntermediateDirectory(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	path := filepath.Join(root, "staging")
	outside := filepath.Join(root, "outside")
	for _, directory := range []string{path, outside} {
		if err := os.Mkdir(directory, 0o700); err != nil {
			t.Fatal(err)
		}
	}
	if err := os.Symlink(outside, filepath.Join(path, "bin")); err != nil {
		t.Fatal(err)
	}
	top, err := sysx.OpenDirectoryAt(sysx.WorkingDirectory, path, path)
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = sysx.Close(top) }()
	failed := stageChild(top, path, plannedNode(t, "bin/run", "payload", true, 0o600))
	if failed == nil || failed.Path != filepath.Join(path, "bin") || failed.Errno == 0 {
		t.Fatalf("staging through a swapped directory = %v, want an OS error at %q", failed, filepath.Join(path, "bin"))
	}
	entries, err := os.ReadDir(outside)
	if err != nil {
		t.Fatal(err)
	}
	if len(entries) != 0 {
		t.Errorf("staging wrote %v through the link", entries)
	}
}

func lstatMetadata(t *testing.T, path string) sysx.Metadata {
	t.Helper()
	metadata, err := sysx.LstatMetadata(path)
	if err != nil {
		t.Fatal(err)
	}
	return metadata
}

func TestReadFileReportsChangesAfterInspection(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	path := filepath.Join(root, "AGENTS.md")
	writeFile(t, path, "before")
	before := lstatMetadata(t, path)
	if err := os.Remove(path); err != nil {
		t.Fatal(err)
	}
	_, failed := readFile(path, before, maxResourceFileBytes, nil)
	wantFailure(t, failed, CodeConcurrentChange, "file disappeared while opening", path)

	if err := os.Symlink(filepath.Join(root, "elsewhere"), path); err != nil {
		t.Fatal(err)
	}
	_, failed = readFile(path, before, maxResourceFileBytes, nil)
	wantFailure(t, failed, CodeConcurrentChange, "file changed while opening", path)
	if err := os.Remove(path); err != nil {
		t.Fatal(err)
	}

	writeFile(t, path, "before")
	grown := func(opened string) {
		file, err := os.OpenFile(opened, os.O_WRONLY|os.O_APPEND, 0)
		if err != nil {
			t.Error(err)
			return
		}
		defer func() { _ = file.Close() }()
		if _, err := file.WriteString(" and after"); err != nil {
			t.Error(err)
		}
	}
	_, failed = readFile(path, lstatMetadata(t, path), maxResourceFileBytes, grown)
	wantFailure(t, failed, CodeConcurrentChange, "file changed while reading", path)
}

func TestObservationWalkReportsChangesDuringTheWalk(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	tree := filepath.Join(root, "tree")
	writeFile(t, filepath.Join(tree, "a.md"), "a")
	added := filepath.Join(tree, "b.md")
	entries := observation{signatures: map[string]sysx.Metadata{}, security: func(path string) ([]byte, *TargetError) {
		if path == filepath.Join(tree, "a.md") {
			writeFile(t, added, "b")
		}
		return []byte{}, nil
	}}
	wantFailure(t, entries.visit(tree, "", 0), CodeConcurrentChange, "directory entries changed", tree)
	if err := os.Remove(added); err != nil {
		t.Fatal(err)
	}

	captures := map[string]int{}
	security := observation{signatures: map[string]sysx.Metadata{}, security: func(path string) ([]byte, *TargetError) {
		captures[path]++
		return []byte(strconv.Itoa(captures[path])), nil
	}}
	wantFailure(t, security.visit(tree, "", 0), CodeConcurrentChange, "security metadata changed", filepath.Join(tree, "a.md"))
}

func TestVerifyObservedComparesEachDirectoryWithItsOwnChildren(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	tree := filepath.Join(root, "tree")
	writeFile(t, filepath.Join(tree, "a.md"), "a")
	writeFile(t, filepath.Join(tree, "sub", "b.md"), "b")
	writeFile(t, filepath.Join(tree, "sub", "deeper", "c.md"), "c")
	walk := observation{signatures: map[string]sysx.Metadata{}}
	if failed := walk.visit(tree, "", 0); failed != nil {
		t.Fatal(failed)
	}
	slices.SortFunc(walk.nodes, func(a, b node) int { return strings.Compare(a.path, b.path) })
	observed := must(newRevision(walk.nodes))
	if failed := verifyObserved(tree, observed, walk.signatures); failed != nil {
		t.Fatalf("unchanged tree: %v", failed)
	}
	if diff := cmp.Diff(map[string][]string{"": {"a.md", "sub"}, "sub": {"b.md", "deeper"}, "sub/deeper": {"c.md"}},
		observedChildren(observed.nodes)); diff != "" {
		t.Errorf("children (-want +got):\n%s", diff)
	}
	writeFile(t, filepath.Join(tree, "sub", "deeper", "d.md"), "d")
	deeper := filepath.Join(tree, "sub", "deeper")
	walk.signatures[deeper] = lstatMetadata(t, deeper)
	wantFailure(t, verifyObserved(tree, observed, walk.signatures), CodeConcurrentChange, "directory entries changed", deeper)
}

func TestDirectoryListingIsNoFollowAndBounded(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	tree := filepath.Join(root, "tree")
	for _, name := range []string{"a", "b", "c", "d", "e"} {
		writeFile(t, filepath.Join(tree, name), name)
	}
	metadata := lstatMetadata(t, tree)
	children, failed := directoryChildren(tree, metadata, 2)
	mustSucceed(t, failed)
	if got := len(children); got != 3 {
		t.Errorf("bounded listing returned %d entries, want 3", got)
	}
	children, failed = directoryChildren(tree, metadata, 5)
	mustSucceed(t, failed)
	if diff := cmp.Diff([]string{"a", "b", "c", "d", "e"}, childNames(children)); diff != "" {
		t.Errorf("listing (-want +got):\n%s", diff)
	}

	link := filepath.Join(root, "link")
	if err := os.Symlink(tree, link); err != nil {
		t.Fatal(err)
	}
	_, failed = directoryChildren(link, metadata, 5)
	wantFailure(t, failed, CodeConcurrentChange, "path changed during inspection", link)
	other := filepath.Join(root, "other")
	if err := os.Mkdir(other, 0o700); err != nil {
		t.Fatal(err)
	}
	_, failed = directoryChildren(other, metadata, 5)
	wantFailure(t, failed, CodeConcurrentChange, "path changed during inspection", other)

	walk := observation{nodes: make([]node, maxResourceNodes-4), signatures: map[string]sysx.Metadata{}}
	wantFailure(t, walk.visit(tree, "", 0), CodeUnsupported,
		"[Errno "+strconv.Itoa(int(syscall.ENOTSUP))+"] resource exceeds the node limit: PosixPath('"+tree+"')", tree)
	if got := len(walk.nodes); got != maxResourceNodes-3 {
		t.Errorf("walk visited %d nodes before refusing the listing, want only the directory", got-(maxResourceNodes-4))
	}
}

func TestDestroyRemovesOnlyTheExpectedRevision(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	path := filepath.Join(root, "tree")
	planned := plannedTree(t, BundleEntry{Path: "a/b.md", Data: []byte("b")})
	written, failed := writeRevision(path, planned)
	mustSucceed(t, failed)
	var unlinked []string
	ctx := seams.With(t.Context(), &seams.Hooks{Observe: func(event seams.Event) {
		if event.Kind == seams.KindUnlink {
			unlinked = append(unlinked, event.Path)
		}
	}})
	writeFile(t, filepath.Join(path, "foreign.md"), "foreign")
	wantFailure(t, destroy(ctx, path, written), CodeRecoveryNeeded, "cleanup revision changed", path)
	if err := os.Remove(filepath.Join(path, "foreign.md")); err != nil {
		t.Fatal(err)
	}
	written = mustObserve(t, path)
	mustSucceed(t, destroy(ctx, path, written))
	if _, err := os.Lstat(path); !errors.Is(err, os.ErrNotExist) {
		t.Fatalf("destroyed tree still exists: %v", err)
	}
	if diff := cmp.Diff([]string{filepath.Join(path, "a", "b.md")}, unlinked); diff != "" {
		t.Errorf("unlink seams (-want +got):\n%s", diff)
	}
	mustSucceed(t, destroy(ctx, path, written))
	if !cleanupRevision(revision{nodes: written.nodes[:1]}, written) || !cleanupRevision(revision{}, written) {
		t.Error("prefixes of the expected revision are not cleanup revisions")
	}
	if cleanupRevision(written, revision{nodes: written.nodes[:1]}) {
		t.Error("a longer revision is a cleanup revision")
	}
}

func TestExclusiveRenamePreservesCollidingDestinations(t *testing.T) {
	t.Parallel()
	for _, kind := range []string{"empty", "nonempty", "file"} {
		t.Run(kind, func(t *testing.T) {
			t.Parallel()
			root := tempRoot(t)
			source := filepath.Join(root, "source")
			destination := filepath.Join(root, "destination")
			writeFile(t, filepath.Join(source, "original"), "source")
			switch kind {
			case "file":
				writeFile(t, destination, "destination")
			case "nonempty":
				writeFile(t, filepath.Join(destination, "original"), "destination")
			default:
				if err := os.Mkdir(destination, 0o700); err != nil {
					t.Fatal(err)
				}
			}
			var events []seams.Event
			ctx := seams.With(t.Context(), &seams.Hooks{Observe: func(event seams.Event) { events = append(events, event) }})
			failed := renameExclusive(ctx, source, destination)
			wantFailure(t, failed, CodeIOError, "[Errno "+strconv.Itoa(int(syscall.EEXIST))+"] File exists: PosixPath('"+destination+"')", destination)
			if failed.Errno != syscall.EEXIST {
				t.Errorf("errno = %v", failed.Errno)
			}
			if data, err := os.ReadFile(filepath.Join(source, "original")); err != nil || string(data) != "source" {
				t.Errorf("source after refusal = %q, %v", data, err)
			}
			final := filepath.Join(root, "final")
			mustSucceed(t, renameExclusive(ctx, source, final))
			if data, err := os.ReadFile(filepath.Join(final, "original")); err != nil || string(data) != "source" {
				t.Errorf("moved payload = %q, %v", data, err)
			}
			want := []seams.Event{
				{Kind: seams.KindRename, Source: source, Destination: destination},
				{Kind: seams.KindRename, Source: source, Destination: final},
				{Kind: seams.KindRename, Source: source, Destination: final, Done: true},
			}
			if diff := cmp.Diff(want, events); diff != "" {
				t.Errorf("rename seams (-want +got):\n%s", diff)
			}
		})
	}
}

func TestMoveRevisionRequiresTheExactRevisionOnBothSides(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	source := filepath.Join(root, "staged")
	written, failed := writeRevision(source, must(desiredRevision(must(NewFileContent([]byte("x"), ModeReadable)))))
	mustSucceed(t, failed)
	destination := filepath.Join(root, "AGENTS.md")
	mustSucceed(t, moveRevision(t.Context(), source, destination, written))
	if data, err := os.ReadFile(destination); err != nil || string(data) != "x" {
		t.Fatalf("moved file = %q, %v", data, err)
	}
	wantFailure(t, moveRevision(t.Context(), destination, source, revision{}), CodeConcurrentChange, "complete resource revision changed", destination)
}

func TestCreateParentsRecordsOnlyDirectoriesItCreated(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	existing := filepath.Join(root, "existing")
	if err := os.Mkdir(existing, 0o700); err != nil {
		t.Fatal(err)
	}
	created, failed := createParents(filepath.Join(existing, "a", "b", "AGENTS.md"), nil)
	mustSucceed(t, failed)
	var paths []string
	for _, item := range created {
		paths = append(paths, item.path)
	}
	want := []string{filepath.Join(existing, "a"), filepath.Join(existing, "a", "b")}
	if diff := cmp.Diff(want, paths); diff != "" {
		t.Errorf("created parents (-want +got):\n%s", diff)
	}
	for _, item := range created {
		if item.mode.small != 0o700 {
			t.Errorf("%s mode = %o, want 700", item.path, item.mode.small)
		}
	}
	again, failed := createParents(filepath.Join(existing, "a", "b", "AGENTS.md"), created)
	mustSucceed(t, failed)
	if !ancestorsEqual(again, created) {
		t.Errorf("repeated createParents changed the record")
	}
	blocker := filepath.Join(root, "blocker")
	writeFile(t, blocker, "file")
	_, failed = createParents(filepath.Join(blocker, "x", "AGENTS.md"), nil)
	if failed == nil || failed.Errno != syscall.ENOTDIR {
		t.Fatalf("createParents below a file = %v, want ENOTDIR", failed)
	}
}

func TestCreateStoreParentsCreatesTheStoreFootprintAndStagingArea(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	location := filepath.Join(root, "outer", "state")
	st := pathStore{MemoryStore: store.NewMemoryStore(), location: location, staging: filepath.Join(location, "staging")}
	created, failed := createStoreParents(st, nil)
	mustSucceed(t, failed)
	var paths []string
	for _, item := range created {
		paths = append(paths, item.path)
	}
	want := []string{filepath.Join(root, "outer"), location, filepath.Join(location, "staging")}
	if diff := cmp.Diff(want, paths); diff != "" {
		t.Errorf("created store parents (-want +got):\n%s", diff)
	}
	mem, failed := createStoreParents(store.NewMemoryStore(), nil)
	mustSucceed(t, failed)
	if len(mem) != 0 {
		t.Errorf("memory store parents = %v", mem)
	}
}

type pathStore struct {
	*store.MemoryStore
	location string
	staging  string
}

func (s pathStore) Location() (string, bool) { return s.location, s.location != "" }

func (s pathStore) StagingDirectory() (string, bool) { return s.staging, s.staging != "" }

func otherDevice(t *testing.T, root string) string {
	t.Helper()
	info, err := os.Stat(root)
	if err != nil {
		t.Fatal(err)
	}
	device := info.Sys().(*syscall.Stat_t).Dev
	for _, candidate := range []string{"/dev", "/proc", "/sys"} {
		other, err := os.Stat(candidate)
		if err == nil && other.IsDir() && other.Sys().(*syscall.Stat_t).Dev != device {
			return candidate
		}
	}
	t.Skip("no directory on another device")
	return ""
}

func TestTransactionDirectoryUsesStagingOnTheSameMountOnly(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	destination := filepath.Join(root, "project", "AGENTS.md")
	if err := os.Mkdir(filepath.Join(root, "project"), 0o700); err != nil {
		t.Fatal(err)
	}
	fallback := must(TransactionPath(destination, testTransaction))
	st, err := store.NewDirectoryStore(t.Context(), filepath.Join(root, "state"))
	if err != nil {
		t.Fatal(err)
	}
	staged := filepath.Join(root, "state", "staging", destinationKey(destination)+"-"+testTransaction)
	cases := []struct {
		name  string
		store store.Store
		want  string
	}{
		{"directory store", st, staged},
		{"memory store", store.NewMemoryStore(), fallback},
		{"other device", pathStore{MemoryStore: store.NewMemoryStore(), staging: filepath.Join(otherDevice(t, root), "flyrail-staging")}, fallback},
	}
	for _, test := range cases {
		directory, failed := chooseTransactionDirectory(test.store, destination, testTransaction)
		mustSucceed(t, failed)
		if directory != test.want {
			t.Errorf("%s: transaction directory = %q, want %q", test.name, directory, test.want)
		}
		if !recognizedTransactionDirectory(test.store, destination, testTransaction, directory) {
			t.Errorf("%s: chosen directory is not recognized", test.name)
		}
		if !recognizedTransactionDirectory(test.store, destination, testTransaction, fallback) {
			t.Errorf("%s: fallback directory is not recognized", test.name)
		}
	}
	if recognizedTransactionDirectory(st, destination, testTransaction, filepath.Join(root, "elsewhere")) {
		t.Error("an unrelated directory is recognized")
	}
	_, failed := chooseTransactionDirectory(st, destination, "short")
	wantFailure(t, failed, CodeInvalidState, "transaction id must be 32 lowercase hexadecimal characters", "")
}

func TestErrorDetailMapsAuthorityFailures(t *testing.T) {
	t.Parallel()
	cases := []struct {
		err   error
		code  ErrorCode
		path  string
		errno syscall.Errno
	}{
		{&AuthorityError{Message: "bad", Err: newValueError("bad")}, CodeInvalidState, "", 0},
		{&AuthorityError{Message: "unsafe", Path: "/x", Err: ErrUnsafePath}, CodeUnsafePath, "/x", 0},
		{&AuthorityError{Message: "io", Path: "/x", Err: syscall.EACCES}, CodeIOError, "/x", syscall.EACCES},
		{&AuthorityError{Message: "io", Path: "/x", Err: syscall.EXDEV}, CodeUnsupported, "/x", syscall.EXDEV},
	}
	for _, test := range cases {
		failed := errorDetail(test.err)
		if failed.Code != test.code || failed.Path != test.path || failed.Errno != test.errno {
			t.Errorf("errorDetail(%v) = %s %q %v", test.err, failed.Code, failed.Path, failed.Errno)
		}
	}
	if failed := planAuthorityError(&AuthorityError{Message: "io", Err: syscall.EXDEV}); failed.Code != CodeIOError {
		t.Errorf("planAuthorityError code = %s, want io_error", failed.Code)
	}
}

func TestHierarchyConflictsAreStoreScoped(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	tree := treeRef(t, filepath.Join(root, "config"))
	child := documentRef(t, filepath.Join(root, "config", "child.md"))
	first, second := store.NewMemoryStore(), store.NewMemoryStore()
	mustSucceed(t, admit(t.Context(), first, tree))
	mustSucceed(t, admit(t.Context(), second, child))
	conflicts, failed := hierarchyConflicts(t.Context(), second, ResourceAuthority{destination: child.Destination}, false)
	mustSucceed(t, failed)
	if len(conflicts) != 0 {
		t.Errorf("conflicts across stores = %v", conflicts)
	}
	mustSucceed(t, writeRecord(t.Context(), first, authorityKey(child), child, nil))
	conflicts, failed = hierarchyConflicts(t.Context(), first, ResourceAuthority{destination: tree.Destination}, true)
	mustSucceed(t, failed)
	if !slices.Equal(conflicts, []string{child.Destination}) {
		t.Errorf("descendant conflicts = %v", conflicts)
	}
}
