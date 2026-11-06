//go:build darwin || linux

package flyrail

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"syscall"
	"testing"
	"time"

	"github.com/google/go-cmp/cmp"

	"github.com/nielsmadan/flyrail/go/flyrail/store"
)

type storeOp int

const (
	opRead storeOp = iota
	opWrite
	opDelete
)

type storeCall struct {
	op    storeOp
	after bool
	key   string
	inner store.Store
}

type hookedStore struct {
	store.Store
	mu   sync.Mutex
	hook func(storeCall) error
}

func hooked(inner store.Store) *hookedStore {
	return &hookedStore{Store: inner}
}

func (s *hookedStore) set(hook func(storeCall) error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.hook = hook
}

func (s *hookedStore) clear() { s.set(nil) }

func (s *hookedStore) call(op storeOp, after bool, key string) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.hook == nil {
		return nil
	}
	return s.hook(storeCall{op: op, after: after, key: key, inner: s.Store})
}

func (s *hookedStore) Read(ctx context.Context, key string) ([]byte, bool, error) {
	if err := s.call(opRead, false, key); err != nil {
		return nil, false, err
	}
	value, ok, err := s.Store.Read(ctx, key)
	if err != nil {
		return nil, false, err
	}
	if err := s.call(opRead, true, key); err != nil {
		return nil, false, err
	}
	return value, ok, nil
}

func (s *hookedStore) Write(ctx context.Context, key string, value []byte, expected store.Expected) (bool, error) {
	if err := s.call(opWrite, false, key); err != nil {
		return false, err
	}
	written, err := s.Store.Write(ctx, key, value, expected)
	if err != nil {
		return false, err
	}
	return written, s.call(opWrite, true, key)
}

func (s *hookedStore) Delete(ctx context.Context, key string, expected []byte) (bool, error) {
	if err := s.call(opDelete, false, key); err != nil {
		return false, err
	}
	deleted, err := s.Store.Delete(ctx, key, expected)
	if err != nil {
		return false, err
	}
	return deleted, s.call(opDelete, true, key)
}

func injectedFailure(message string) error {
	return &store.IOError{Message: message, Errno: syscall.EIO}
}

type crashPoint struct {
	name   string
	op     storeOp
	after  bool
	suffix string
}

var (
	afterPreparation        = crashPoint{"after-preparation", opWrite, true, "/preparation"}
	afterStaging            = crashPoint{"after-staging", opWrite, false, "/journal"}
	beforeCommit            = crashPoint{"before-commit", opWrite, false, "/receipt"}
	afterCommit             = crashPoint{"after-commit", opWrite, true, "/receipt"}
	beforeJournalDelete     = crashPoint{"before-journal-delete", opDelete, false, "/journal"}
	beforePreparationDelete = crashPoint{"before-preparation-delete", opDelete, false, "/preparation"}
)

func (c crashPoint) matches(call storeCall) bool {
	return call.op == c.op && call.after == c.after && strings.HasSuffix(call.key, c.suffix)
}

func crashOnce(c crashPoint) func(storeCall) error {
	fired := false
	return func(call storeCall) error {
		if !fired && c.matches(call) {
			fired = true
			panic(interruption{at: c.name})
		}
		return nil
	}
}

func crashed(t *testing.T, st *hookedStore, c crashPoint, operation func()) {
	t.Helper()
	st.set(crashOnce(c))
	defer st.clear()
	interrupted(t, operation)
}

func hookedDirectory(t *testing.T, root string) *hookedStore {
	t.Helper()
	return hooked(openStore(t, filepath.Join(root, "state")))
}

func teamRendered(t *testing.T, destination, text string) *RenderedBundle {
	t.Helper()
	return sectionRendered(t, destination, text)
}

func transactionDirectories(t *testing.T, destination string, st store.Store) []string {
	t.Helper()
	var result []string
	parent := filepath.Dir(destination)
	for _, name := range directoryNames(t, parent) {
		folded := strings.ToLower(name)
		if strings.HasPrefix(folded, ".flyrail-") && strings.HasSuffix(folded, ".txn") {
			result = append(result, filepath.Join(parent, name))
		}
	}
	if st != nil {
		if staging, ok := st.StagingDirectory(); ok && exists(t, staging) {
			for _, name := range directoryNames(t, staging) {
				result = append(result, filepath.Join(staging, name))
			}
		}
	}
	slices.Sort(result)
	return result
}

func recordedPreparation(t *testing.T, st store.Store, destination string) *preparation {
	t.Helper()
	prepared, ok, failed := readRecord(t.Context(), st, preparationKey(documentRef(t, destination)), "Preparation", preparationFromState)
	mustSucceed(t, failed)
	if !ok {
		return nil
	}
	return &prepared
}

func journalDirectory(t *testing.T, st store.Store, destination string) string {
	t.Helper()
	prepared := recordedPreparation(t, st, destination)
	if prepared == nil {
		t.Fatal("no preparation record")
	}
	return prepared.directory
}

type treeItem struct {
	Path string
	Mode os.FileMode
	Data string
}

func treeState(t *testing.T, directory string) []treeItem {
	t.Helper()
	var items []treeItem
	err := filepath.WalkDir(directory, func(path string, entry os.DirEntry, err error) error {
		if err != nil || path == directory {
			return err
		}
		info, err := entry.Info()
		if err != nil {
			return err
		}
		item := treeItem{Path: strings.TrimPrefix(path, directory+"/"), Mode: info.Mode()}
		if info.Mode().IsRegular() {
			data, err := os.ReadFile(path)
			if err != nil {
				return err
			}
			item.Data = string(data)
		}
		items = append(items, item)
		return nil
	})
	if err != nil {
		t.Fatal(err)
	}
	return items
}

func recoveryPathsOf(observation InstallationObservation) [][]string {
	paths := make([][]string, 0, len(observation.Resources))
	for _, resource := range observation.Resources {
		paths = append(paths, resource.RecoveryPaths)
	}
	return paths
}

func resultRecoveryPaths(result InstallationResult) [][]string {
	paths := make([][]string, 0, len(result.Resources))
	for _, resource := range result.Resources {
		paths = append(paths, resource.RecoveryPaths)
	}
	return paths
}

func noRecoveryPaths(result InstallationResult) bool {
	return !slices.ContainsFunc(result.Resources, func(item ResourceResult) bool { return len(item.RecoveryPaths) > 0 })
}

func TestNothingPersistentIsLeftBesideTheDestination(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	selected := stateTarget(t, root)
	destination := filepath.Join(root, "AGENTS.md")
	writeFile(t, destination, "foreign\n")
	steps := []func() InstallationResult{
		func() InstallationResult {
			return syncOnce(t, teamBundle(t, "1"), teamRendered(t, destination, "one"), selected)
		},
		func() InstallationResult {
			return syncOnce(t, teamBundle(t, "2"), teamRendered(t, destination, "two"), selected)
		},
		func() InstallationResult { return removeAll(t, selected, false) },
	}
	for index, step := range steps {
		wantStatus(t, "step", step(), StatusApplied)
		if diff := cmp.Diff([]string{"AGENTS.md", "state"}, directoryNames(t, root)); diff != "" {
			t.Errorf("%d: root (-want +got):\n%s", index, diff)
		}
		if leftover := transactionDirectories(t, destination, selected.Store()); len(leftover) != 0 {
			t.Errorf("%d: transaction directories %v", index, leftover)
		}
	}
	if got := readText(t, destination); got != "foreign\n" {
		t.Errorf("destination = %q", got)
	}
}

func TestAnUnwritableStoreFailsWithoutPanicking(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	selected := stateTarget(t, root)
	destination := filepath.Join(root, "AGENTS.md")
	if err := os.Mkdir(filepath.Join(root, "state"), 0o500); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = os.Chmod(filepath.Join(root, "state"), 0o700) })
	result := syncOnce(t, teamBundle(t, "1"), teamRendered(t, destination, "generated"), selected)
	wantStatus(t, "sync", result, StatusFailed)
	wantCode(t, "sync", result.Err, CodeIOError)
	if exists(t, destination) {
		t.Error("the destination was written")
	}
}

func TestAStoreOthersCanWriteFailsWithoutPanicking(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	location := filepath.Join(root, "state")
	if err := os.Mkdir(location, 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.Chmod(location, 0o777); err != nil {
		t.Fatal(err)
	}
	result := syncOnce(t, teamBundle(t, "1"), teamRendered(t, filepath.Join(root, "AGENTS.md"), "generated"), targetOf(t, openStore(t, location)))
	wantStatus(t, "sync", result, StatusFailed)
	wantCode(t, "sync", result.Err, CodeIOError)
	if result.Err.Path != location {
		t.Errorf("error path = %q, want %q", result.Err.Path, location)
	}
	if diff := cmp.Diff([]string{"state"}, directoryNames(t, root)); diff != "" {
		t.Errorf("root (-want +got):\n%s", diff)
	}
	if names := directoryNames(t, location); len(names) != 0 {
		t.Errorf("store holds %v", names)
	}
}

func TestAStoreFailureAtEachProtocolPointKeepsTheDestinationConsistent(t *testing.T) {
	t.Parallel()
	for _, fragment := range []string{"index/", "/authority", "/preparation", "/journal", "/receipt"} {
		root := tempRoot(t)
		st := hookedDirectory(t, root)
		selected := targetOf(t, st)
		destination := filepath.Join(root, "AGENTS.md")
		second := destination
		if fragment == "/authority" {
			second = filepath.Join(root, "CLAUDE.md")
		}
		appliedSync(t, teamBundle(t, "1"), teamRendered(t, destination, "one"), selected)
		before := readText(t, destination)
		st.set(func(call storeCall) error {
			if call.op == opWrite && !call.after && strings.Contains(call.key, fragment) {
				return injectedFailure("injected store failure")
			}
			return nil
		})
		failed := syncOnce(t, teamBundle(t, "2"), teamRendered(t, second, "two"), selected)
		st.clear()
		if failed.Status != StatusFailed && failed.Status != StatusIncomplete {
			t.Errorf("%s: status = %s", fragment, failed.Status)
		}
		if readText(t, destination) != before {
			t.Errorf("%s: the destination changed", fragment)
		}
		if leftover := transactionDirectories(t, destination, st); len(leftover) != 0 {
			t.Errorf("%s: transaction directories %v", fragment, leftover)
		}
		if second != destination && exists(t, second) {
			t.Errorf("%s: the second destination was written", fragment)
		}
		appliedSync(t, teamBundle(t, "2"), teamRendered(t, second, "two"), selected)
	}
}

func TestAnInterruptedTransactionIsReportedAndRecovered(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	st := hookedDirectory(t, root)
	selected := targetOf(t, st)
	destination := filepath.Join(root, "AGENTS.md")
	crashed(t, st, beforeCommit, func() { syncOnce(t, teamBundle(t, "1"), teamRendered(t, destination, "generated"), selected) })
	pending := journalDirectory(t, st, destination)
	if diff := cmp.Diff([][]string{{pending}}, recoveryPathsOf(inspectTeam(t, selected))); diff != "" {
		t.Errorf("recovery paths (-want +got):\n%s", diff)
	}
	appliedSync(t, teamBundle(t, "1"), teamRendered(t, destination, "generated"), selected)
	if diff := cmp.Diff([]string{"AGENTS.md", "state"}, directoryNames(t, root)); diff != "" {
		t.Errorf("root (-want +got):\n%s", diff)
	}
}

func TestCleanupInterruptedAfterRemovingTransactionDirectoriesIsRecovered(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	st := hookedDirectory(t, root)
	selected := targetOf(t, st)
	destination := filepath.Join(root, "AGENTS.md")
	appliedSync(t, teamBundle(t, "1"), teamRendered(t, destination, "one"), selected)
	crashed(t, st, beforeJournalDelete, func() { syncOnce(t, teamBundle(t, "2"), teamRendered(t, destination, "two"), selected) })
	pending := journalDirectory(t, st, destination)
	if exists(t, pending) {
		t.Fatal("the transaction directory remains")
	}
	if diff := cmp.Diff([]string{pending}, inspectTeam(t, selected).Resources[0].RecoveryPaths); diff != "" {
		t.Errorf("recovery paths (-want +got):\n%s", diff)
	}
	wantStatus(t, "recover", recoverAll(t, selected), StatusApplied)
	if hasRecord(t, st, journalKey(documentRef(t, destination))) {
		t.Error("the journal remains")
	}
	if leftover := transactionDirectories(t, destination, st); len(leftover) != 0 {
		t.Errorf("transaction directories %v", leftover)
	}
	if !strings.Contains(readText(t, destination), "two") {
		t.Error("the update was not published")
	}
	appliedSync(t, teamBundle(t, "2"), teamRendered(t, destination, "two"), selected)
}

func TestATransactionDirectoryWithoutAJournalIsLeftUntouched(t *testing.T) {
	t.Parallel()
	for _, layout := range [][]string{{}, {"staging/"}, {"staging/", "backup/"}, {"staging/", "backup/", "staging/resource"}} {
		root := tempRoot(t)
		selected := stateTarget(t, root)
		destination := filepath.Join(root, "AGENTS.md")
		appliedSync(t, teamBundle(t, "1"), teamRendered(t, destination, "one"), selected)
		unrecorded := must(TransactionPath(destination, strings.Repeat("f", 32)))
		if err := os.Mkdir(unrecorded, 0o700); err != nil {
			t.Fatal(err)
		}
		var expected []string
		for _, name := range layout {
			if directory, ok := strings.CutSuffix(name, "/"); ok {
				if err := os.Mkdir(filepath.Join(unrecorded, directory), 0o700); err != nil {
					t.Fatal(err)
				}
				expected = append(expected, directory)
				continue
			}
			exclusiveWrite(t, filepath.Join(unrecorded, name), "retained", 0o600)
			expected = append(expected, name)
		}
		if paths := inspectTeam(t, selected).Resources[0].RecoveryPaths; len(paths) != 0 {
			t.Errorf("%v: recovery paths %v", layout, paths)
		}
		wantStatus(t, "recover", recoverAll(t, selected), StatusUnchanged)
		appliedSync(t, teamBundle(t, "2"), teamRendered(t, destination, "two"), selected)
		if !strings.Contains(readText(t, destination), "two") {
			t.Errorf("%v: the update was not published", layout)
		}
		if diff := cmp.Diff([]string{unrecorded}, transactionDirectories(t, destination, nil)); diff != "" {
			t.Errorf("%v: transaction directories (-want +got):\n%s", layout, diff)
		}
		var got []string
		for _, item := range treeState(t, unrecorded) {
			got = append(got, item.Path)
		}
		slices.Sort(expected)
		if diff := cmp.Diff(expected, got); diff != "" {
			t.Errorf("%v: retained layout (-want +got):\n%s", layout, diff)
		}
	}
}

func TestACrashBeforeTheJournalIsCleanedUpByTheNextRun(t *testing.T) {
	t.Parallel()
	for _, crash := range []crashPoint{afterPreparation, afterStaging} {
		for _, directoryBacked := range []bool{true, false} {
			root := tempRoot(t)
			st := hooked(store.NewMemoryStore())
			if directoryBacked {
				st = hookedDirectory(t, root)
			}
			selected := targetOf(t, st)
			destination := filepath.Join(root, "AGENTS.md")
			writeFile(t, destination, "# Foreign\n")
			appliedSync(t, teamBundle(t, "1"), teamRendered(t, destination, "one"), selected)
			crashed(t, st, crash, func() { syncOnce(t, teamBundle(t, "2"), teamRendered(t, destination, "two"), selected) })
			recorded := recordedPreparation(t, st, destination)
			if recorded == nil {
				t.Fatalf("%s: no preparation", crash.name)
			}
			if hasRecord(t, st, journalKey(documentRef(t, destination))) {
				t.Errorf("%s: a journal was recorded", crash.name)
			}
			if got := exists(t, recorded.directory); got != (crash == afterStaging) {
				t.Errorf("%s: transaction directory exists = %v", crash.name, got)
			}
			if diff := cmp.Diff([]string{recorded.directory}, inspectTeam(t, selected).Resources[0].RecoveryPaths); diff != "" {
				t.Errorf("%s: recovery paths (-want +got):\n%s", crash.name, diff)
			}
			appliedSync(t, teamBundle(t, "2"), teamRendered(t, destination, "two"), selected)
			if recordedPreparation(t, st, destination) != nil {
				t.Errorf("%s: the preparation remains", crash.name)
			}
			want := []string{"AGENTS.md"}
			if directoryBacked {
				want = append(want, "state")
			}
			if diff := cmp.Diff(want, directoryNames(t, root)); diff != "" {
				t.Errorf("%s: root (-want +got):\n%s", crash.name, diff)
			}
			if leftover := transactionDirectories(t, destination, st); len(leftover) != 0 {
				t.Errorf("%s: transaction directories %v", crash.name, leftover)
			}
		}
	}
}

func victim(t *testing.T, root string) string {
	t.Helper()
	directory := filepath.Join(root, "victim")
	for _, folder := range []string{directory, filepath.Join(directory, "staging"), filepath.Join(directory, "backup")} {
		if err := os.Mkdir(folder, 0o700); err != nil {
			t.Fatal(err)
		}
	}
	exclusiveWrite(t, filepath.Join(directory, "staging", "resource"), "victim", 0o600)
	return directory
}

func damage(t *testing.T, pending, root, kind string) string {
	t.Helper()
	switch kind {
	case "symlink":
		moved := filepath.Join(root, "victim")
		if err := os.Rename(pending, moved); err != nil {
			t.Fatal(err)
		}
		if err := os.Symlink(moved, pending); err != nil {
			t.Fatal(err)
		}
		return filepath.Join(moved, "staging", "resource")
	case "staging-link":
		moved := victim(t, root)
		for _, path := range []string{filepath.Join(pending, "staging", "resource"), filepath.Join(pending, "staging")} {
			if err := os.Remove(path); err != nil {
				t.Fatal(err)
			}
		}
		if err := os.Symlink(filepath.Join(moved, "staging"), filepath.Join(pending, "staging")); err != nil {
			t.Fatal(err)
		}
		return filepath.Join(moved, "staging", "resource")
	case "nonprivate":
		if err := os.Chmod(pending, 0o755); err != nil {
			t.Fatal(err)
		}
		return filepath.Join(pending, "staging", "resource")
	}
	exclusiveWrite(t, filepath.Join(pending, kind), "retained", 0o600)
	return filepath.Join(pending, kind)
}

func TestUnrecognizedDataBesideAPreparationIsRetained(t *testing.T) {
	t.Parallel()
	for _, kind := range []string{"unknown", "backup/resource", "symlink", "staging-link", "nonprivate"} {
		root := tempRoot(t)
		st := hookedDirectory(t, root)
		selected := targetOf(t, st)
		destination := filepath.Join(root, "AGENTS.md")
		appliedSync(t, teamBundle(t, "1"), teamRendered(t, destination, "one"), selected)
		before := readText(t, destination)
		crashed(t, st, afterStaging, func() { syncOnce(t, teamBundle(t, "2"), teamRendered(t, destination, "two"), selected) })
		key := preparationKey(documentRef(t, destination))
		recorded := storeValue(t, st, key)
		pending := journalDirectory(t, st, destination)
		evidence := damage(t, pending, root, kind)
		retained := readText(t, evidence)
		result := recoverAll(t, selected)
		wantStatus(t, kind, result, StatusIncomplete)
		wantCode(t, kind, result.Err, CodeRecoveryNeeded)
		if result.Err.Path != pending {
			t.Errorf("%s: error path = %q, want %q", kind, result.Err.Path, pending)
		}
		if diff := cmp.Diff([][]string{{pending}}, resultRecoveryPaths(result)); diff != "" {
			t.Errorf("%s: recovery paths (-want +got):\n%s", kind, diff)
		}
		if string(storeValue(t, st, key)) != string(recorded) {
			t.Errorf("%s: the preparation changed", kind)
		}
		if readText(t, evidence) != retained || readText(t, destination) != before {
			t.Errorf("%s: retained data changed", kind)
		}
		_ = os.Chmod(pending, 0o700)
	}
}

func TestAJournalWithoutItsPreparationIsRetainedAsInvalidState(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	st := hookedDirectory(t, root)
	selected := targetOf(t, st)
	destination := filepath.Join(root, "AGENTS.md")
	appliedSync(t, teamBundle(t, "1"), teamRendered(t, destination, "one"), selected)
	crashed(t, st, beforeCommit, func() { syncOnce(t, teamBundle(t, "2"), teamRendered(t, destination, "two"), selected) })
	pending := journalDirectory(t, st, destination)
	reference := documentRef(t, destination)
	recorded := storeValue(t, st, preparationKey(reference))
	if deleted, err := st.Delete(t.Context(), preparationKey(reference), recorded); err != nil || !deleted {
		t.Fatalf("delete preparation: %v %v", deleted, err)
	}
	journalBytes := storeValue(t, st, journalKey(reference))
	published := readText(t, destination)
	layout := treeState(t, pending)
	if !strings.Contains(published, "two") {
		t.Fatal("the crash happened before publication")
	}
	observation := inspectTeam(t, selected)
	recovered := recoverAll(t, selected)
	synced := syncOnce(t, teamBundle(t, "2"), teamRendered(t, destination, "two"), selected)
	wantCode(t, "inspection", observation.Resources[0].Err, CodeInvalidState)
	for _, result := range []InstallationResult{recovered, synced} {
		wantStatus(t, "lone journal", result, StatusIncomplete)
		wantCode(t, "lone journal", result.Err, CodeInvalidState)
	}
	if string(storeValue(t, st, journalKey(reference))) != string(journalBytes) {
		t.Error("the journal changed")
	}
	if diff := cmp.Diff(layout, treeState(t, pending)); diff != "" {
		t.Errorf("transaction directory (-before +after):\n%s", diff)
	}
	if readText(t, destination) != published {
		t.Error("the destination changed")
	}
}

func TestCleanupRemovesThePreparationAfterTheJournal(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	selected := stateTarget(t, root)
	appliedSync(t, teamBundle(t, "1"), teamRendered(t, filepath.Join(root, "AGENTS.md"), "generated"), selected)
	for _, key := range must(selected.Store().Keys(t.Context(), "resource/")) {
		if strings.HasSuffix(key, "/journal") || strings.HasSuffix(key, "/preparation") {
			t.Errorf("transaction record %s remains", key)
		}
	}
}

func TestACleanupInterruptedBeforeDeletingThePreparationIsRecovered(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	st := hookedDirectory(t, root)
	selected := targetOf(t, st)
	destination := filepath.Join(root, "AGENTS.md")
	appliedSync(t, teamBundle(t, "1"), teamRendered(t, destination, "one"), selected)
	crashed(t, st, beforePreparationDelete, func() { syncOnce(t, teamBundle(t, "2"), teamRendered(t, destination, "two"), selected) })
	if hasRecord(t, st, journalKey(documentRef(t, destination))) || recordedPreparation(t, st, destination) == nil {
		t.Fatal("the crash did not leave only the preparation")
	}
	wantStatus(t, "recover", recoverAll(t, selected), StatusApplied)
	if recordedPreparation(t, st, destination) != nil {
		t.Error("the preparation remains")
	}
	if !strings.Contains(readText(t, destination), "two") {
		t.Error("the update was not published")
	}
	if diff := cmp.Diff([]string{"AGENTS.md", "state"}, directoryNames(t, root)); diff != "" {
		t.Errorf("root (-want +got):\n%s", diff)
	}
	if leftover := transactionDirectories(t, destination, st); len(leftover) != 0 {
		t.Errorf("transaction directories %v", leftover)
	}
}

func TestALostStoreMakesInstalledContentForeign(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	destination := filepath.Join(root, "AGENTS.md")
	appliedSync(t, teamBundle(t, "1"), teamRendered(t, destination, "generated"), stateTarget(t, root))
	replacement := stateTarget(t, filepath.Join(root, "replacement"))
	conflicted := syncOnce(t, teamBundle(t, "1"), teamRendered(t, destination, "generated"), replacement)
	wantStatus(t, "conflict", conflicted, StatusFailed)
	codes := []ErrorCode{}
	if conflicted.Err != nil {
		codes = append(codes, conflicted.Err.Code)
	}
	for _, item := range conflicted.Resources {
		if item.Err != nil {
			codes = append(codes, item.Err.Code)
		}
	}
	if !slices.Contains(codes, CodeConflict) {
		t.Errorf("codes = %v", codes)
	}
	adopted := syncWith(t.Context(), t, teamBundle(t, "1"), teamRendered(t, destination, "generated"), replacement, SyncOptions{Acquisition: AcquireAdopt, LockTimeout: testLockTimeout})
	wantStatus(t, "adopt", adopted, StatusApplied)
}

func TestAnotherStoreLeavesALiveTransactionDirectoryUntouched(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	destination := filepath.Join(root, "AGENTS.md")
	writeFile(t, destination, "# project\n")
	first := hooked(store.NewMemoryStore())
	second := stateTarget(t, filepath.Join(root, "state-b"))
	type observed struct {
		directory string
		contents  []treeItem
		result    InstallationResult
	}
	var seen []observed
	first.set(func(call storeCall) error {
		if call.op != opWrite || !call.after || !strings.HasSuffix(call.key, "/journal") {
			return nil
		}
		recorded, _, err := call.inner.Read(t.Context(), preparationKey(documentRef(t, destination)))
		if err != nil {
			return err
		}
		prepared := must(decodeState(recorded, preparationFromState))
		before := treeState(t, prepared.directory)
		result := syncOnce(t, instructionsBundle(t, "other", "1", "notes"), renderedOf(t, sectionAt(t, "notes", destination, "from B")), second)
		if diff := cmp.Diff(before, treeState(t, prepared.directory)); diff != "" {
			t.Errorf("the live transaction directory changed (-before +after):\n%s", diff)
		}
		seen = append(seen, observed{directory: prepared.directory, contents: before, result: result})
		return nil
	})
	result := syncOnce(t, teamBundle(t, "1"), teamRendered(t, destination, "from A"), targetOf(t, first))
	first.clear()
	if len(seen) != 1 {
		t.Fatalf("the second store ran %d times", len(seen))
	}
	if filepath.Dir(seen[0].directory) != root || len(seen[0].contents) == 0 {
		t.Errorf("transaction directory %s holds %v", seen[0].directory, seen[0].contents)
	}
	wantStatus(t, "second store", seen[0].result, StatusApplied)
	if !noRecoveryPaths(seen[0].result) {
		t.Errorf("second store recovery paths %v", resultRecoveryPaths(seen[0].result))
	}
	if !strings.Contains(readText(t, destination), "from B") {
		t.Error("the second store's content was not published")
	}
	wantCode(t, "first store", result.Err, CodeConcurrentChange)
	wantStatus(t, "recover", must(RecoverInstallation(t.Context(), "other", second, LockOptions{LockTimeout: testLockTimeout})), StatusUnchanged)
}

func treeRendered(t *testing.T, path, text string) *RenderedBundle {
	t.Helper()
	return renderedOf(t, must(NewRenderedArtifact(RenderedArtifactParams{
		ID:          "guide",
		Family:      FamilyInstructions,
		Destination: path,
		Content:     must(NewTreeContent([]BundleEntry{{Path: "a.txt", Data: []byte(text)}})),
	})))
}

func TestAStoreReadFailureAfterStagingDiscardsTheStagedRevision(t *testing.T) {
	t.Parallel()
	for _, kind := range []string{"document", "tree"} {
		root := tempRoot(t)
		st := hookedDirectory(t, root)
		selected := targetOf(t, st)
		destination := filepath.Join(root, "out")
		desired := func(text string) *RenderedBundle { return treeRendered(t, destination, text) }
		snapshot := func() []treeItem { return treeState(t, destination) }
		if kind == "document" {
			destination = filepath.Join(root, "AGENTS.md")
			desired = func(text string) *RenderedBundle { return teamRendered(t, destination, text) }
			snapshot = func() []treeItem { return []treeItem{{Data: readText(t, destination)}} }
		}
		appliedSync(t, teamBundle(t, "1"), desired("one"), selected)
		before := snapshot()
		armed := false
		st.set(func(call storeCall) error {
			if call.op == opWrite && call.after && strings.HasSuffix(call.key, "/preparation") {
				armed = true
			}
			if armed && call.op == opRead && strings.HasSuffix(call.key, "/authority") {
				return injectedFailure("injected store read failure")
			}
			return nil
		})
		failed := syncOnce(t, teamBundle(t, "2"), desired("two"), selected)
		st.clear()
		wantStatus(t, kind, failed, StatusFailed)
		wantCode(t, kind, failed.Err, CodeIOError)
		if diff := cmp.Diff(before, snapshot()); diff != "" {
			t.Errorf("%s: destination changed (-before +after):\n%s", kind, diff)
		}
		if leftover := transactionDirectories(t, destination, st); len(leftover) != 0 {
			t.Errorf("%s: transaction directories %v", kind, leftover)
		}
		reference := documentRef(t, destination)
		if kind == "tree" {
			reference = treeRef(t, destination)
		}
		if hasRecord(t, st, preparationKey(reference)) {
			t.Errorf("%s: the preparation remains", kind)
		}
		appliedSync(t, teamBundle(t, "2"), desired("two"), selected)
		published := destination
		if kind == "tree" {
			published = filepath.Join(destination, "a.txt")
		}
		if !strings.Contains(readText(t, published), "two") {
			t.Errorf("%s: the update was not published", kind)
		}
	}
}

func recordListings(st *hookedStore, project string, crash *crashPoint) *[][]string {
	listings := &[][]string{}
	fired := false
	st.set(func(call storeCall) error {
		if call.op == opWrite || call.op == opDelete {
			entries, _ := os.ReadDir(project)
			names := make([]string, 0, len(entries))
			for _, entry := range entries {
				names = append(names, entry.Name())
			}
			*listings = append(*listings, names)
		}
		if crash != nil && !fired && crash.matches(call) {
			fired = true
			panic(interruption{at: crash.name})
		}
		return nil
	})
	return listings
}

func TestASameFilesystemStoreNeverCreatesAnythingBesideTheDestination(t *testing.T) {
	t.Parallel()
	for _, crash := range []crashPoint{afterPreparation, afterStaging, afterCommit} {
		root := tempRoot(t)
		project := filepath.Join(root, "project")
		mkdir(t, project)
		destination := filepath.Join(project, "AGENTS.md")
		writeFile(t, destination, "# Foreign\n")
		st := hookedDirectory(t, root)
		selected := targetOf(t, st)
		install := recordListings(st, project, nil)
		appliedSync(t, teamBundle(t, "1"), teamRendered(t, destination, "one"), selected)
		interruptedListings := recordListings(st, project, &crash)
		interrupted(t, func() { syncOnce(t, teamBundle(t, "2"), teamRendered(t, destination, "two"), selected) })
		afterCrash := directoryNames(t, project)
		recovery := recordListings(st, project, nil)
		appliedSync(t, teamBundle(t, "2"), teamRendered(t, destination, "two"), selected)
		st.clear()
		all := slices.Concat(*install, *interruptedListings, *recovery)
		if len(all) == 0 {
			t.Fatalf("%s: no store mutation was observed", crash.name)
		}
		for _, listing := range append(all, afterCrash) {
			if slices.ContainsFunc(listing, func(name string) bool { return name != "AGENTS.md" }) {
				t.Errorf("%s: project held %v", crash.name, listing)
			}
		}
		if diff := cmp.Diff([]string{"AGENTS.md"}, directoryNames(t, project)); diff != "" {
			t.Errorf("%s: project (-want +got):\n%s", crash.name, diff)
		}
		if leftover := transactionDirectories(t, destination, st); len(leftover) != 0 {
			t.Errorf("%s: transaction directories %v", crash.name, leftover)
		}
		if info, err := os.Stat(filepath.Join(root, "state", "staging")); err != nil || !info.IsDir() {
			t.Errorf("%s: the staging directory is missing: %v", crash.name, err)
		}
		if !strings.Contains(readText(t, destination), "two") {
			t.Errorf("%s: the update was not published", crash.name)
		}
	}
}

func captureTransactionDirectories(t *testing.T, st *hookedStore, destination string) *[]string {
	t.Helper()
	directories := &[]string{}
	st.set(func(call storeCall) error {
		if call.op != opWrite || call.after || !strings.HasSuffix(call.key, "/journal") {
			return nil
		}
		recorded, _, err := call.inner.Read(t.Context(), preparationKey(documentRef(t, destination)))
		if err != nil {
			return err
		}
		prepared := must(decodeState(recorded, preparationFromState))
		if info, err := os.Stat(prepared.directory); err != nil || !info.IsDir() {
			t.Errorf("transaction directory %s is missing: %v", prepared.directory, err)
		}
		*directories = append(*directories, prepared.directory)
		return nil
	})
	return directories
}

func TestAStoreWithoutAStagingDirectoryFallsBackBesideTheDestination(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	destination := filepath.Join(root, "AGENTS.md")
	st := hooked(store.NewMemoryStore())
	directories := captureTransactionDirectories(t, st, destination)
	appliedSync(t, teamBundle(t, "1"), teamRendered(t, destination, "generated"), targetOf(t, st))
	st.clear()
	if len(*directories) != 1 || filepath.Dir((*directories)[0]) != root {
		t.Fatalf("transaction directories %v", *directories)
	}
	name := filepath.Base((*directories)[0])
	if !strings.HasPrefix(name, ".flyrail-") || !strings.HasSuffix(name, ".txn") {
		t.Errorf("transaction directory name %q", name)
	}
	if diff := cmp.Diff([]string{"AGENTS.md"}, directoryNames(t, root)); diff != "" {
		t.Errorf("root (-want +got):\n%s", diff)
	}
}

func TestASameFilesystemStoreStagesInItsStagingDirectory(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	destination := filepath.Join(root, "AGENTS.md")
	st := hookedDirectory(t, root)
	directories := captureTransactionDirectories(t, st, destination)
	appliedSync(t, teamBundle(t, "1"), teamRendered(t, destination, "generated"), targetOf(t, st))
	st.clear()
	staging := filepath.Join(root, "state", "staging")
	if len(*directories) != 1 || filepath.Dir((*directories)[0]) != staging {
		t.Fatalf("transaction directories %v", *directories)
	}
	if !strings.HasPrefix(filepath.Base((*directories)[0]), destinationKey(destination)+"-") {
		t.Errorf("transaction directory %s is not named for its destination", (*directories)[0])
	}
	if names := directoryNames(t, staging); len(names) != 0 {
		t.Errorf("staging holds %v", names)
	}
}

func TestAPreparationNamingAForeignDirectoryIsRefused(t *testing.T) {
	t.Parallel()
	for _, named := range []string{"elsewhere", "another-transaction", "another-destination"} {
		root := tempRoot(t)
		st := hookedDirectory(t, root)
		selected := targetOf(t, st)
		destination := filepath.Join(root, "AGENTS.md")
		appliedSync(t, teamBundle(t, "1"), teamRendered(t, destination, "one"), selected)
		before := readText(t, destination)
		crashed(t, st, afterStaging, func() { syncOnce(t, teamBundle(t, "2"), teamRendered(t, destination, "two"), selected) })
		key := preparationKey(documentRef(t, destination))
		recorded := recordedPreparation(t, st, destination)
		decoy := filepath.Join(root, "decoy")
		switch named {
		case "another-transaction":
			decoy = filepath.Join(filepath.Dir(recorded.directory), destinationKey(destination)+"-"+strings.Repeat("f", 32))
		case "another-destination":
			decoy = must(TransactionPath(filepath.Join(root, "CLAUDE.md"), recorded.transactionID))
		}
		if err := os.Rename(victim(t, root), decoy); err != nil {
			t.Fatal(err)
		}
		corrupted := encodeRecord(must(newPreparation(recorded.resource, recorded.transactionID, decoy, stateSchemaVersion)))
		replaceRecord(t, st, key, corrupted)
		for _, result := range []InstallationResult{
			recoverAll(t, selected),
			syncOnce(t, teamBundle(t, "2"), teamRendered(t, destination, "two"), selected),
		} {
			wantStatus(t, named, result, StatusIncomplete)
			wantCode(t, named, result.Err, CodeRecoveryNeeded)
			if !noRecoveryPaths(result) {
				t.Errorf("%s: recovery paths %v", named, resultRecoveryPaths(result))
			}
		}
		if string(storeValue(t, st, key)) != string(corrupted) {
			t.Errorf("%s: the preparation changed", named)
		}
		if readText(t, filepath.Join(decoy, "staging", "resource")) != "victim" || readText(t, destination) != before {
			t.Errorf("%s: retained data changed", named)
		}
	}
}

func crashThenMoveStore(t *testing.T, root string, crash crashPoint) (string, preparation) {
	t.Helper()
	st := hookedDirectory(t, root)
	selected := targetOf(t, st)
	destination := filepath.Join(root, "AGENTS.md")
	appliedSync(t, teamBundle(t, "1"), teamRendered(t, destination, "one"), selected)
	crashed(t, st, crash, func() { syncOnce(t, teamBundle(t, "2"), teamRendered(t, destination, "two"), selected) })
	recorded := recordedPreparation(t, st, destination)
	if recorded == nil || hasRecord(t, st, journalKey(documentRef(t, destination))) {
		t.Fatal("the crash did not leave only a preparation")
	}
	if err := os.Rename(filepath.Join(root, "state"), filepath.Join(root, "moved")); err != nil {
		t.Fatal(err)
	}
	return destination, *recorded
}

func movedTarget(t *testing.T, root string) *InstallationTarget {
	t.Helper()
	return targetOf(t, openStore(t, filepath.Join(root, "moved")))
}

func TestAPreparationLeftBehindByAMovedStoreIsReleased(t *testing.T) {
	t.Parallel()
	for _, recoverFirst := range []bool{true, false} {
		root := tempRoot(t)
		destination, recorded := crashThenMoveStore(t, root, afterPreparation)
		moved := movedTarget(t, root)
		if _, err := os.Lstat(recorded.directory); !errors.Is(err, os.ErrNotExist) {
			t.Fatalf("the old transaction directory exists: %v", err)
		}
		if recoverFirst {
			wantStatus(t, "recover", recoverAll(t, moved), StatusApplied)
			if recordedPreparation(t, moved.Store(), destination) != nil {
				t.Error("recovery kept the preparation")
			}
		}
		appliedSync(t, teamBundle(t, "2"), teamRendered(t, destination, "two"), moved)
		if recordedPreparation(t, moved.Store(), destination) != nil {
			t.Error("the preparation remains")
		}
		if !strings.Contains(readText(t, destination), "two") {
			t.Error("the update was not published")
		}
		if leftover := transactionDirectories(t, destination, moved.Store()); len(leftover) != 0 {
			t.Errorf("transaction directories %v", leftover)
		}
	}
}

func TestAMovedStoreRefusesAPreparationWhoseOldDirectoryStillExists(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	destination, recorded := crashThenMoveStore(t, root, afterStaging)
	moved := movedTarget(t, root)
	before := readText(t, destination)
	if err := os.MkdirAll(filepath.Dir(recorded.directory), 0o700); err != nil {
		t.Fatal(err)
	}
	staging, _ := moved.Store().StagingDirectory()
	if err := os.Rename(filepath.Join(staging, filepath.Base(recorded.directory)), recorded.directory); err != nil {
		t.Fatal(err)
	}
	retained := treeState(t, recorded.directory)
	key := preparationKey(documentRef(t, destination))
	original := storeValue(t, moved.Store(), key)
	for _, result := range []InstallationResult{
		recoverAll(t, moved),
		syncOnce(t, teamBundle(t, "2"), teamRendered(t, destination, "two"), moved),
	} {
		wantStatus(t, "moved store", result, StatusIncomplete)
		wantCode(t, "moved store", result.Err, CodeRecoveryNeeded)
		if !noRecoveryPaths(result) {
			t.Errorf("recovery paths %v", resultRecoveryPaths(result))
		}
	}
	if string(storeValue(t, moved.Store(), key)) != string(original) {
		t.Error("the preparation changed")
	}
	if diff := cmp.Diff(retained, treeState(t, recorded.directory)); diff != "" {
		t.Errorf("old transaction directory (-before +after):\n%s", diff)
	}
	if readText(t, destination) != before {
		t.Error("the destination changed")
	}
}

func TestAStagingDirectoryAnotherWriterCreatesBeforeLockingIsNotApproved(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	selected := stateTarget(t, root)
	destination := filepath.Join(root, "AGENTS.md")
	proposal := previewOf(t, teamBundle(t, "1"), teamRendered(t, destination, "generated"), selected)
	for _, directory := range []string{filepath.Join(root, "state"), filepath.Join(root, "state", "staging")} {
		if err := os.Mkdir(directory, 0o700); err != nil {
			t.Fatal(err)
		}
	}
	result := ApplyPreview(t.Context(), proposal, LockOptions{LockTimeout: testLockTimeout})
	wantStatus(t, "apply", result, StatusFailed)
	wantCode(t, "apply", result.Err, CodeConcurrentChange)
	if exists(t, destination) || !exists(t, filepath.Join(root, "state", "staging")) {
		t.Error("the foreign staging directory was approved")
	}
}

func TestAParentRecreatedDuringTheLockWaitIsNotApproved(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	location := filepath.Join(root, "new", "state")
	st := openStore(t, location)
	selected := targetOf(t, st)
	destination := filepath.Join(root, "AGENTS.md")
	key := installationKey("team", selected)
	held := must(st.Lock(t.Context(), key, 0))
	proposal := previewOf(t, teamBundle(t, "1"), teamRendered(t, destination, "generated"), selected)
	staging := filepath.Join(location, "staging")
	if exists(t, staging) {
		t.Fatal("preview created the staging directory")
	}
	done := make(chan InstallationResult, 1)
	go func() { done <- ApplyPreview(t.Context(), proposal, LockOptions{LockTimeout: 20 * time.Second}) }()
	deadline := time.Now().Add(20 * time.Second)
	for !exists(t, staging) {
		if time.Now().After(deadline) {
			t.Fatal("apply never created the staging directory")
		}
		select {
		case result := <-done:
			t.Fatalf("apply finished before creating staging: %+v", result.Err)
		case <-time.After(2 * time.Millisecond):
		}
	}
	select {
	case result := <-done:
		t.Fatalf("apply finished without the index lock: %+v", result.Err)
	case <-time.After(200 * time.Millisecond):
	}
	if err := os.Rename(staging, filepath.Join(root, "replaced")); err != nil {
		t.Fatal(err)
	}
	if err := os.Mkdir(staging, 0o500); err != nil {
		t.Fatal(err)
	}
	if err := held.Unlock(); err != nil {
		t.Fatal(err)
	}
	result := <-done
	wantStatus(t, "apply", result, StatusFailed)
	wantCode(t, "apply", result.Err, CodeConcurrentChange)
	if exists(t, destination) || hasRecord(t, st, key) || hasRecord(t, st, receiptKey(documentRef(t, destination))) {
		t.Error("the unapproved parent was used")
	}
}

func TestAnUnreadableJournalReportsOnlyTheDirectoryItsPreparationNames(t *testing.T) {
	t.Parallel()
	for _, c := range []struct {
		unreadable []string
		reported   bool
	}{{[]string{"/journal"}, true}, {[]string{"/journal", "/preparation"}, false}} {
		root := tempRoot(t)
		st := hookedDirectory(t, root)
		selected := targetOf(t, st)
		destination := filepath.Join(root, "AGENTS.md")
		appliedSync(t, teamBundle(t, "1"), teamRendered(t, destination, "one"), selected)
		var directories []string
		armed := false
		st.set(func(call storeCall) error {
			if armed && call.op == opRead && slices.ContainsFunc(c.unreadable, func(suffix string) bool { return strings.HasSuffix(call.key, suffix) }) {
				return injectedFailure("injected store read failure")
			}
			if call.op == opWrite && strings.HasSuffix(call.key, "/journal") {
				if !call.after {
					recorded, _, err := call.inner.Read(t.Context(), preparationKey(documentRef(t, destination)))
					if err != nil {
						return err
					}
					directories = append(directories, must(decodeState(recorded, preparationFromState)).directory)
				} else {
					armed = true
				}
			}
			return nil
		})
		result := syncOnce(t, teamBundle(t, "2"), teamRendered(t, destination, "two"), selected)
		st.clear()
		if len(directories) == 0 || filepath.Dir(directories[0]) != filepath.Join(root, "state", "staging") {
			t.Fatalf("transaction directories %v", directories)
		}
		want := [][]string{nil}
		status := StatusFailed
		if c.reported {
			want, status = [][]string{{directories[0]}}, StatusIncomplete
		}
		if diff := cmp.Diff(want, resultRecoveryPaths(result)); diff != "" {
			t.Errorf("%v: recovery paths (-want +got):\n%s", c.unreadable, diff)
		}
		wantStatus(t, "unreadable journal", result, status)
		wantStatus(t, "recover", recoverAll(t, selected), StatusApplied)
		appliedSync(t, teamBundle(t, "2"), teamRendered(t, destination, "two"), selected)
		if !strings.Contains(readText(t, destination), "two") {
			t.Errorf("%v: the update was not published", c.unreadable)
		}
	}
}

func otherGroup(t *testing.T) (int, bool) {
	t.Helper()
	groups, err := os.Getgroups()
	if err != nil {
		t.Fatal(err)
	}
	primary := os.Getegid()
	for _, group := range groups {
		if group != primary {
			return group, true
		}
	}
	return 0, false
}

func TestAStoreOwnedByAnotherGroupStagesFilesWithTheIntendedGroup(t *testing.T) {
	t.Parallel()
	group, ok := otherGroup(t)
	if !ok {
		t.Skip("the user belongs to a single group")
	}
	root := tempRoot(t)
	location := filepath.Join(root, "state")
	if err := os.Mkdir(location, 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.Chown(location, -1, group); err != nil {
		t.Fatal(err)
	}
	destination := filepath.Join(root, "owned")
	appliedSync(t, teamBundle(t, "1"), renderedOf(t, fileAt(t, "guide", destination, "whole\n")), targetOf(t, openStore(t, location)))
	if got := readText(t, destination); got != "whole\n" {
		t.Errorf("destination = %q", got)
	}
}
