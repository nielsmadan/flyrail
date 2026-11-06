//go:build darwin || linux

package flyrail

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/google/go-cmp/cmp"

	"github.com/nielsmadan/flyrail/go/flyrail/store"
	"github.com/nielsmadan/flyrail/go/internal/jsonx"
	"github.com/nielsmadan/flyrail/go/internal/native"
	"github.com/nielsmadan/flyrail/go/internal/pathx"
	"github.com/nielsmadan/flyrail/go/internal/seams"
)

const testContextDigest = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"

var testIndexKey = indexKey("team", testContextDigest)

func fileRevision(t *testing.T, data string) revision {
	t.Helper()
	return must(newRevision([]node{must(newPlannedNode("", []byte(data), true, 0o600))}))
}

func exclusiveWrite(t *testing.T, path, data string, mode os.FileMode) {
	t.Helper()
	file, err := os.OpenFile(path, os.O_WRONLY|os.O_CREATE|os.O_EXCL, mode)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := file.WriteString(data); err != nil {
		t.Fatal(err)
	}
	if err := file.Close(); err != nil {
		t.Fatal(err)
	}
}

func readText(t *testing.T, path string) string {
	t.Helper()
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	return string(data)
}

func exists(t *testing.T, path string) bool {
	t.Helper()
	_, err := os.Lstat(path)
	if err != nil && !errors.Is(err, os.ErrNotExist) {
		t.Fatal(err)
	}
	return err == nil
}

func testReceipt(t *testing.T, reference ResourceRef, transaction byte) receipt {
	t.Helper()
	return must(newReceipt(reference, strings.Repeat(string(transaction), 32), nil, DocumentProvenance{}, nil, stateSchemaVersion))
}

func putReceipt(t *testing.T, st store.Store, reference ResourceRef, recorded receipt) {
	t.Helper()
	mustSucceed(t, writeRecord(t.Context(), st, receiptKey(reference), recorded, nil))
}

func receiptOf(t *testing.T, st store.Store, reference ResourceRef) *receipt {
	t.Helper()
	recorded, failed := readReceipt(t.Context(), st, reference)
	mustSucceed(t, failed)
	return recorded
}

func hasRecord(t *testing.T, st store.Store, key string) bool {
	t.Helper()
	_, ok, err := st.Read(t.Context(), key)
	if err != nil {
		t.Fatal(err)
	}
	return ok
}

func testPlan(t *testing.T, st store.Store, reference ResourceRef, before, after revision, previous *receipt, next receipt) ResourcePlan {
	t.Helper()
	directory, failed := chooseTransactionDirectory(st, reference.Destination, next.transactionID)
	mustSucceed(t, failed)
	captured, failed := resourceAncestors(reference.Destination, pathx.Join(directory, "staging"))
	mustSucceed(t, failed)
	recoveryPaths, failed := pendingRecovery(t.Context(), st, reference)
	mustSucceed(t, failed)
	return must(newResourcePlan(ResourcePlan{
		resource:             reference,
		before:               before,
		after:                after,
		previous:             previous,
		receipt:              next,
		ancestors:            captured,
		transactionDirectory: directory,
		recoveryPaths:        recoveryPaths,
	}))
}

func lockOne(t *testing.T, st store.Store, reference ResourceRef) *resourceLock {
	t.Helper()
	held, failed := acquireResourceLock(t.Context(), st, reference, 0)
	mustSucceed(t, failed)
	t.Cleanup(func() { _ = held.Unlock() })
	return held
}

func admitted(t *testing.T, st store.Store, reference ResourceRef) ResourceRef {
	t.Helper()
	mustSucceed(t, admit(t.Context(), st, reference))
	return reference
}

func managementEntries(t *testing.T, st store.Store, destination string) []string {
	t.Helper()
	var entries []string
	parent := filepath.Dir(destination)
	names, err := os.ReadDir(parent)
	if err != nil {
		t.Fatal(err)
	}
	for _, entry := range names {
		if native.ManagementName(entry.Name()) {
			entries = append(entries, filepath.Join(parent, entry.Name()))
		}
	}
	if staging, ok := st.StagingDirectory(); ok {
		staged, err := os.ReadDir(staging)
		if err != nil && !errors.Is(err, os.ErrNotExist) {
			t.Fatal(err)
		}
		for _, entry := range staged {
			entries = append(entries, filepath.Join(staging, entry.Name()))
		}
	}
	return entries
}

func assertSettled(t *testing.T, st store.Store, reference ResourceRef) {
	t.Helper()
	recoveryPaths, failed := pendingRecovery(t.Context(), st, reference)
	mustSucceed(t, failed)
	if len(recoveryPaths) > 0 {
		t.Errorf("recovery paths = %v", recoveryPaths)
	}
	if entries := managementEntries(t, st, reference.Destination); len(entries) > 0 {
		t.Errorf("transaction entries = %v", entries)
	}
}

type interruption struct{ at string }

func crashWhen(when func(seams.Event) bool) *seams.Hooks {
	fired := false
	return &seams.Hooks{Observe: func(event seams.Event) {
		if !fired && when(event) {
			fired = true
			panic(interruption{at: event.Boundary})
		}
	}}
}

func atBoundary(name string) func(seams.Event) bool {
	return func(event seams.Event) bool { return event.Kind == seams.KindBoundary && event.Boundary == name }
}

func parentIs(path, name string) bool {
	return filepath.Base(filepath.Dir(path)) == name
}

func renameInto(area string, done bool) func(seams.Event) bool {
	return func(event seams.Event) bool {
		return event.Kind == seams.KindRename && event.Done == done && parentIs(event.Destination, area)
	}
}

func renameOutOf(area string, done bool) func(seams.Event) bool {
	return func(event seams.Event) bool {
		return event.Kind == seams.KindRename && event.Done == done && parentIs(event.Source, area)
	}
}

func interrupted(t *testing.T, operation func()) {
	t.Helper()
	crashed := func() (crashed bool) {
		defer func() {
			if value := recover(); value != nil {
				if _, ok := value.(interruption); !ok {
					panic(value)
				}
				crashed = true
			}
		}()
		operation()
		return false
	}()
	if !crashed {
		t.Fatal("the operation was not interrupted")
	}
}

func faultAt(boundaries ...string) *seams.Hooks {
	return &seams.Hooks{Fault: func(boundary string) bool { return slices.Contains(boundaries, boundary) }}
}

func faultAfter(boundary string, successful int) *seams.Hooks {
	remaining := successful
	return &seams.Hooks{Fault: func(selected string) bool {
		if selected != boundary {
			return false
		}
		if remaining > 0 {
			remaining--
			return false
		}
		return true
	}}
}

type transactionSetup struct {
	st        store.Store
	locks     *transactionLocks
	plans     []ResourcePlan
	previous  *index
	pending   index
	completed index
}

func storedIndex(t *testing.T, st store.Store) *index {
	t.Helper()
	recorded, ok, failed := readRecord(t.Context(), st, testIndexKey, "Index", indexFromState)
	mustSucceed(t, failed)
	if !ok {
		return nil
	}
	return &recorded
}

// The locks are released at cleanup or by finish.
func newTransaction(t *testing.T, st store.Store, plans ...ResourcePlan) transactionSetup {
	t.Helper()
	references := make([]ResourceRef, 0, len(plans))
	for _, plan := range plans {
		references = append(references, plan.resource)
	}
	locks, failed := acquireTransactionLocks(t.Context(), st, testIndexKey, references, 0, nil)
	mustSucceed(t, failed)
	t.Cleanup(func() { _ = locks.Unlock() })
	previous := storedIndex(t, st)
	return transactionSetup{
		st:        st,
		locks:     locks,
		plans:     plans,
		previous:  previous,
		pending:   must(pendingIndex("team", testContextDigest, previous, nil, references)),
		completed: must(completedIndex("team", testContextDigest, nil)),
	}
}

func (s transactionSetup) execute(ctx context.Context, t *testing.T) transactionOutcome {
	t.Helper()
	outcome, failed := executeTransaction(ctx, s.locks, s.plans, s.previous, s.pending, s.completed)
	mustSucceed(t, failed)
	return outcome
}

func (s transactionSetup) finish(t *testing.T) {
	t.Helper()
	if err := s.locks.Unlock(); err != nil {
		t.Fatal(err)
	}
}

func (s transactionSetup) storedIndex(t *testing.T) *index {
	t.Helper()
	return storedIndex(t, s.st)
}

func TestPreparePublishAndCleanupCommitAResourceAndReceipt(t *testing.T) {
	t.Parallel()
	st := store.NewMemoryStore()
	reference := admitted(t, st, documentRef(t, filepath.Join(tempRoot(t), "AGENTS.md")))
	next := testReceipt(t, reference, '1')
	plan := testPlan(t, st, reference, mustObserve(t, reference.Destination), fileRevision(t, "generated\n"), nil, next)
	guard := lockOne(t, st, reference)
	prepared, failed := prepare(t.Context(), st, plan, guard)
	mustSucceed(t, failed)
	if !hasRecord(t, st, journalKey(reference)) {
		t.Fatal("prepare did not record the journal")
	}
	committed, failed := publish(t.Context(), st, prepared)
	mustSucceed(t, failed)
	if !committed {
		t.Error("publish did not report its commit")
	}
	if got := readText(t, reference.Destination); got != "generated\n" {
		t.Errorf("destination = %q", got)
	}
	if got := receiptOf(t, st, reference); got == nil || !sameRecord(*got, next) {
		t.Errorf("receipt = %+v", got)
	}
	mustSucceed(t, cleanup(t.Context(), st, prepared))
	assertSettled(t, st, reference)
}

func TestRecoverRollsBackPreparedChangesAndExactPrefixCleanup(t *testing.T) {
	t.Parallel()
	st := store.NewMemoryStore()
	destination := filepath.Join(tempRoot(t), "AGENTS.md")
	writeFile(t, destination, "before\n")
	reference := admitted(t, st, documentRef(t, destination))
	before := mustObserve(t, destination)
	plan := testPlan(t, st, reference, before, fileRevision(t, "after\n"), nil, testReceipt(t, reference, '2'))
	guard := lockOne(t, st, reference)
	prepared, failed := prepare(t.Context(), st, plan, guard)
	mustSucceed(t, failed)
	staged, _ := transactionPaths(plan.transactionDirectory)
	if !cleanupRevision(mustObserve(t, staged), prepared.staged) {
		t.Fatal("staged revision is not a cleanup prefix of the journal")
	}
	mustSucceed(t, recoverResource(t.Context(), st, guard))
	if got := mustObserve(t, destination); !got.equal(before) {
		t.Error("recovery did not restore the original revision")
	}
	assertSettled(t, st, reference)
}

func preparationBoundary(t *testing.T, directory string) time.Time {
	t.Helper()
	info, err := os.Lstat(directory)
	if err != nil {
		t.Fatal(err)
	}
	return info.ModTime()
}

func laterThanBoundary(t *testing.T, path, directory string) {
	t.Helper()
	later := preparationBoundary(t, directory).Add(time.Minute)
	if err := os.Chtimes(path, later, later); err != nil {
		t.Fatal(err)
	}
}

type replacementCase struct {
	st        store.Store
	reference ResourceRef
	old       receipt
	plan      ResourcePlan
	guard     *resourceLock
	prepared  journal
}

func preparedReplacement(t *testing.T, st store.Store, destination string) replacementCase {
	t.Helper()
	exclusiveWrite(t, destination, "before\n", 0o600)
	reference := admitted(t, st, documentRef(t, destination))
	old := testReceipt(t, reference, '0')
	putReceipt(t, st, reference, old)
	plan := testPlan(t, st, reference, mustObserve(t, destination), fileRevision(t, "after\n"), &old, testReceipt(t, reference, '1'))
	guard := lockOne(t, st, reference)
	prepared, failed := prepare(t.Context(), st, plan, guard)
	mustSucceed(t, failed)
	return replacementCase{st: st, reference: reference, old: old, plan: plan, guard: guard, prepared: prepared}
}

func TestRecoverRefusesAReplacementOfThePublishedRevision(t *testing.T) {
	t.Parallel()
	destination := filepath.Join(tempRoot(t), "AGENTS.md")
	c := preparedReplacement(t, store.NewMemoryStore(), destination)
	staged, backup := transactionPaths(c.plan.transactionDirectory)
	mustSucceed(t, moveRevision(t.Context(), destination, backup, c.prepared.backup))
	mustSucceed(t, moveRevision(t.Context(), staged, destination, c.prepared.staged))
	laterThanBoundary(t, destination, c.plan.transactionDirectory)
	failed := recoverResource(t.Context(), c.st, c.guard)
	wantFailure(t, failed, CodeRecoveryNeeded, "destination revision changed after preparation", destination)
	if got := readText(t, destination); got != "after\n" {
		t.Errorf("destination = %q", got)
	}
	if got := receiptOf(t, c.st, c.reference); got == nil || !sameRecord(*got, c.old) {
		t.Errorf("receipt = %+v, want the previous receipt retained", got)
	}
	if !hasRecord(t, c.st, journalKey(c.reference)) || !exists(t, backup) {
		t.Error("journal or backup was not retained")
	}
}

func TestRecoverRefusesAReplacementOfThePreviousRevision(t *testing.T) {
	t.Parallel()
	destination := filepath.Join(tempRoot(t), "AGENTS.md")
	c := preparedReplacement(t, store.NewMemoryStore(), destination)
	laterThanBoundary(t, destination, c.plan.transactionDirectory)
	failed := recoverResource(t.Context(), c.st, c.guard)
	wantFailure(t, failed, CodeRecoveryNeeded, "destination revision changed after preparation", destination)
	if got := readText(t, destination); got != "before\n" {
		t.Errorf("destination = %q", got)
	}
	staged, _ := transactionPaths(c.plan.transactionDirectory)
	if !hasRecord(t, c.st, journalKey(c.reference)) || !exists(t, staged) {
		t.Error("journal or staging was not retained")
	}
}

func TestResourceLocksAreAcquiredInDestinationOrder(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	st := store.NewMemoryStore()
	first, second := documentRef(t, filepath.Join(root, "z")), documentRef(t, filepath.Join(root, "a"))
	locks, failed := lockResources(t.Context(), st, []ResourceRef{first, second}, 0)
	mustSucceed(t, failed)
	t.Cleanup(func() { _ = unlockResources(locks) })
	order := []ResourceRef{locks[0].reference, locks[1].reference}
	if diff := cmp.Diff([]ResourceRef{second, first}, order); diff != "" {
		t.Errorf("lock order (-want +got):\n%s", diff)
	}
	_, failed = acquireResourceLock(t.Context(), st, first, 0)
	if failed == nil || failed.Code != CodeBusy {
		t.Fatalf("second acquisition = %v, want busy", failed)
	}
}

func fixtureRevisionBytes(t *testing.T, value jsonx.Value) string {
	t.Helper()
	path := []string{"fields", "nodes"}
	for _, key := range path {
		value, _ = value.Get(key)
	}
	nodes, ok := value.Items()
	if !ok || len(nodes) == 0 {
		t.Fatal("fixture revision has no nodes")
	}
	data := nodes[0]
	for _, key := range []string{"fields", "data", "bytes"} {
		data, _ = data.Get(key)
	}
	text, ok := data.AsString()
	if !ok {
		t.Fatal("fixture node has no bytes")
	}
	return string(fromHex(t, text))
}

func fixtureMode(t *testing.T, value jsonx.Value) os.FileMode {
	t.Helper()
	for _, key := range []string{"fields", "nodes"} {
		value, _ = value.Get(key)
	}
	nodes, _ := value.Items()
	mode := nodes[0]
	for _, key := range []string{"fields", "mode"} {
		mode, _ = mode.Get(key)
	}
	result, ok := mode.AsInt()
	if !ok {
		t.Fatal("fixture node has no mode")
	}
	return os.FileMode(result)
}

func fixtureAt(t *testing.T, item jsonx.Value, keys ...string) jsonx.Value {
	t.Helper()
	for _, key := range keys {
		var ok bool
		if item, ok = item.Get(key); !ok {
			t.Fatalf("fixture field %s is missing", key)
		}
	}
	return item
}

func TestSharedTransactionVectorsDriveGuardedRecovery(t *testing.T) {
	t.Parallel()
	cases := configurations(t).cases(t, "transactions")
	var names []string
	for _, item := range cases {
		names = append(names, text(t, item, "name"))
	}
	want := []string{"prepared", "backup-moved", "published-before-receipt", "ownership-only-adoption", "receipt-committed",
		"foreign-edit-before-commit", "permissions-changed-before-commit", "identity-changed-before-commit", "committed-foreign-edit"}
	if diff := cmp.Diff(want, names); diff != "" {
		t.Fatalf("transaction vectors (-want +got):\n%s", diff)
	}
	foreign := fixtureRevisionBytes(t, fixtureAt(t, cases[0], "observed", "resource"))
	generated := fixtureRevisionBytes(t, fixtureAt(t, cases[0], "observed", "staged"))
	for _, item := range cases {
		name := text(t, item, "name")
		t.Run(name, func(t *testing.T) {
			t.Parallel()
			root := tempRoot(t)
			destination := filepath.Join(root, "AGENTS.md")
			initial := foreign
			if text(t, item, "journal") == "adopt-section" {
				initial = fixtureRevisionBytes(t, fixtureAt(t, item, "observed", "resource"))
			}
			exclusiveWrite(t, destination, initial, 0o600)
			st := store.NewMemoryStore()
			reference := admitted(t, st, documentRef(t, destination))
			old := testReceipt(t, reference, '0')
			putReceipt(t, st, reference, old)
			before := mustObserve(t, destination)
			next := testReceipt(t, reference, '1')
			after := fileRevision(t, generated)
			if name == "ownership-only-adoption" {
				after = before
			}
			plan := testPlan(t, st, reference, before, after, &old, next)
			guard := lockOne(t, st, reference)
			prepared, failed := prepare(t.Context(), st, plan, guard)
			mustSucceed(t, failed)
			staged, backup := transactionPaths(plan.transactionDirectory)
			observed := func() string { return fixtureRevisionBytes(t, fixtureAt(t, item, "observed", "resource")) }
			switch name {
			case "backup-moved":
				mustSucceed(t, moveRevision(t.Context(), destination, backup, prepared.backup))
			case "published-before-receipt", "foreign-edit-before-commit", "permissions-changed-before-commit", "identity-changed-before-commit":
				mustSucceed(t, moveRevision(t.Context(), destination, backup, prepared.backup))
				mustSucceed(t, moveRevision(t.Context(), staged, destination, prepared.staged))
				switch name {
				case "foreign-edit-before-commit":
					if err := os.WriteFile(destination, []byte(observed()), 0o600); err != nil {
						t.Fatal(err)
					}
				case "permissions-changed-before-commit":
					if err := os.Chmod(destination, fixtureMode(t, fixtureAt(t, item, "observed", "resource"))); err != nil {
						t.Fatal(err)
					}
				case "identity-changed-before-commit":
					replacement := filepath.Join(root, "AGENTS.md.replacement")
					exclusiveWrite(t, replacement, observed(), 0o600)
					if err := os.Rename(replacement, destination); err != nil {
						t.Fatal(err)
					}
				}
			case "receipt-committed", "committed-foreign-edit":
				_, failed := publish(t.Context(), st, prepared)
				mustSucceed(t, failed)
				if name == "committed-foreign-edit" {
					if err := os.WriteFile(destination, []byte(observed()), 0o600); err != nil {
						t.Fatal(err)
					}
				}
			}
			expected := fixtureAt(t, item, "expected")
			status, _ := fixtureAt(t, expected, "status").AsString()
			retainJournal, _ := fixtureAt(t, expected, "retain_journal").AsBool()
			retainBackup, _ := fixtureAt(t, expected, "retain_backup").AsBool()
			recovered := recoverResource(t.Context(), st, guard)
			if (recovered != nil) != (status == "INCOMPLETE") {
				t.Errorf("recovery = %v, want status %s", recovered, status)
			}
			if got := hasRecord(t, st, journalKey(reference)); got != retainJournal {
				t.Errorf("journal retained = %v", got)
			}
			if got := exists(t, backup); got != retainBackup {
				t.Errorf("backup retained = %v", got)
			}
			if got, want := readText(t, destination), fixtureRevisionBytes(t, fixtureAt(t, expected, "resource")); got != want {
				t.Errorf("destination = %q, want %q", got, want)
			}
			wantReceipt := old
			if text(t, expected, "receipt") == "new" {
				wantReceipt = next
			}
			if got := receiptOf(t, st, reference); got == nil || !sameRecord(*got, wantReceipt) {
				t.Errorf("receipt = %+v, want %+v", got, wantReceipt)
			}
		})
	}
}

func TestJournalBindsCreatedDirectoriesToStagedPhysicalNodes(t *testing.T) {
	t.Parallel()
	st := store.NewMemoryStore()
	destination := filepath.Join(tempRoot(t), "skills")
	reference := admitted(t, st, ResourceRef{Destination: authority(t, destination).Destination(), Kind: ResourceSkillContainer})
	after := must(newRevision([]node{
		plannedNode(t, "", "", false, 0o755),
		plannedNode(t, "review", "", false, 0o755),
		plannedNode(t, "review/SKILL.md", "# Review\n", true, 0o644),
	}))
	marker := after.nodes[1]
	next := must(newReceipt(reference, strings.Repeat("3", 32), nil, DocumentProvenance{}, []node{marker}, stateSchemaVersion))
	plan := testPlan(t, st, reference, revision{}, after, nil, next)
	prepared, failed := prepare(t.Context(), st, plan, lockOne(t, st, reference))
	mustSucceed(t, failed)
	index := slices.IndexFunc(prepared.staged.nodes, func(item node) bool { return item.path == "review" })
	physical := prepared.staged.nodes[index]
	if got := prepared.receipt.createdDirectories; len(got) != 1 || !got[0].equal(physical) {
		t.Errorf("created directories = %+v, want the staged node %+v", got, physical)
	}
	if physical.inode == marker.inode {
		t.Error("the marker kept its planned identity")
	}
}

func TestTransactionPublishesIndexBeforeResourcesAndCompletesLast(t *testing.T) {
	t.Parallel()
	st := store.NewMemoryStore()
	reference := admitted(t, st, documentRef(t, filepath.Join(tempRoot(t), "AGENTS.md")))
	plan := testPlan(t, st, reference, mustObserve(t, reference.Destination), fileRevision(t, "generated\n"), nil, testReceipt(t, reference, '4'))
	setup := newTransaction(t, st, plan)
	var duringPublication *index
	hooks := &seams.Hooks{Observe: func(event seams.Event) {
		if event.Kind == seams.KindRename && !event.Done && duringPublication == nil {
			duringPublication = setup.storedIndex(t)
		}
	}}
	outcome := setup.execute(seams.With(t.Context(), hooks), t)
	if diff := cmp.Diff([]ResourceRef{reference}, outcome.completed); diff != "" || len(outcome.unchanged) > 0 || outcome.err != nil {
		t.Fatalf("outcome = %+v", outcome)
	}
	if duringPublication == nil || !sameRecord(*duringPublication, setup.pending) {
		t.Errorf("index during publication = %+v, want the pending index", duringPublication)
	}
	if got := setup.storedIndex(t); got == nil || !sameRecord(*got, setup.completed) {
		t.Errorf("index = %+v, want the completed index", got)
	}
	if got := readText(t, reference.Destination); got != "generated\n" {
		t.Errorf("destination = %q", got)
	}
}

func TestIndexCompletionFailureRetainsAppliedResourceOutcomes(t *testing.T) {
	t.Parallel()
	st := store.NewMemoryStore()
	reference := admitted(t, st, documentRef(t, filepath.Join(tempRoot(t), "AGENTS.md")))
	plan := testPlan(t, st, reference, mustObserve(t, reference.Destination), fileRevision(t, "generated\n"), nil, testReceipt(t, reference, '5'))
	setup := newTransaction(t, st, plan)
	outcome := setup.execute(seams.With(t.Context(), faultAt(seams.IndexCompletion)), t)
	if diff := cmp.Diff([]ResourceRef{reference}, outcome.completed); diff != "" || outcome.failed != nil {
		t.Fatalf("outcome = %+v", outcome)
	}
	wantFailure(t, outcome.err, CodeIOError, "injected index-completion fault", "")
	if got := setup.storedIndex(t); got == nil || !sameRecord(*got, setup.pending) {
		t.Errorf("index = %+v, want the pending index", got)
	}
	if got := readText(t, reference.Destination); got != "generated\n" {
		t.Errorf("destination = %q", got)
	}
}

func TestFailuresAfterTheReceiptCommitPreserveTheCommittedResource(t *testing.T) {
	t.Parallel()
	for _, boundary := range []string{seams.Cleanup, seams.ReceiptReread, seams.ReceiptCommitted} {
		t.Run(boundary, func(t *testing.T) {
			t.Parallel()
			st := store.NewMemoryStore()
			reference := admitted(t, st, documentRef(t, filepath.Join(tempRoot(t), "AGENTS.md")))
			next := testReceipt(t, reference, '6')
			plan := testPlan(t, st, reference, mustObserve(t, reference.Destination), fileRevision(t, "generated\n"), nil, next)
			setup := newTransaction(t, st, plan)
			outcome := setup.execute(seams.With(t.Context(), faultAt(boundary)), t)
			if diff := cmp.Diff([]ResourceRef{reference}, outcome.completed); diff != "" || outcome.failed == nil || *outcome.failed != reference {
				t.Fatalf("outcome = %+v", outcome)
			}
			message := "injected " + boundary + " fault"
			if boundary == seams.Cleanup {
				message += "; recovery: " + message
			}
			wantFailure(t, outcome.err, CodeIOError, message, "")
			if got := receiptOf(t, st, reference); got == nil || !sameRecord(*got, next) {
				t.Errorf("receipt = %+v", got)
			}
			if got := readText(t, reference.Destination); got != "generated\n" {
				t.Errorf("destination = %q", got)
			}
			mustSucceed(t, recoverResource(t.Context(), st, setup.locks.resources[0]))
			assertSettled(t, st, reference)
		})
	}
}

func twoDocuments(t *testing.T, st store.Store, root string) (first, second ResourceRef, plans []ResourcePlan) {
	t.Helper()
	first = admitted(t, st, documentRef(t, filepath.Join(root, "first")))
	second = admitted(t, st, documentRef(t, filepath.Join(root, "second")))
	plans = []ResourcePlan{
		testPlan(t, st, first, mustObserve(t, first.Destination), fileRevision(t, "one\n"), nil, testReceipt(t, first, '7')),
		testPlan(t, st, second, mustObserve(t, second.Destination), fileRevision(t, "two\n"), nil, testReceipt(t, second, '8')),
	}
	return first, second, plans
}

func TestLaterPreconditionFailureRetainsEarlierCommit(t *testing.T) {
	t.Parallel()
	for _, boundary := range []string{seams.ResourcePrecondition, seams.IndexRequire, seams.GuardLookup, seams.PublishReceipt} {
		t.Run(boundary, func(t *testing.T) {
			t.Parallel()
			st := store.NewMemoryStore()
			first, second, plans := twoDocuments(t, st, tempRoot(t))
			setup := newTransaction(t, st, plans...)
			outcome := setup.execute(seams.With(t.Context(), faultAfter(boundary, 1)), t)
			if diff := cmp.Diff([]ResourceRef{first}, outcome.completed); diff != "" || outcome.failed == nil || *outcome.failed != second {
				t.Fatalf("outcome = %+v", outcome)
			}
			wantFailure(t, outcome.err, CodeIOError, "injected "+boundary+" fault", "")
			if got := readText(t, first.Destination); got != "one\n" {
				t.Errorf("first = %q", got)
			}
			if exists(t, second.Destination) {
				t.Error("the second resource was published")
			}
			if got := setup.storedIndex(t); got == nil || !sameRecord(*got, setup.pending) {
				t.Errorf("index = %+v, want the pending index retained", got)
			}
			assertSettled(t, st, second)
		})
	}
}

type stagingStore struct {
	fakeStore
	staging string
}

func (s stagingStore) StagingDirectory() (string, bool) { return s.staging, true }

func TestPreparationRecordsNameTheirDirectoryFirst(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	_, artifact := renderedFile(t, filepath.Join(root, "guide"), "x")
	reference := admittedResource(t, store.NewMemoryStore(), artifact)
	directory := filepath.Join(root, "staging", "transaction")
	encoded := string(encodeRecord(must(newPreparation(reference, testTransaction, directory, stateSchemaVersion))))
	if want := preparationDirectoryPrefix + `"` + directory + `"`; !strings.HasPrefix(encoded, want) {
		t.Errorf("preparation record %s does not start with %s", encoded, want)
	}
}

func TestUnencodableStagingMatchesPythonsPreparationEncodingError(t *testing.T) {
	t.Parallel()
	for _, test := range []struct{ staging, want string }{
		{"/missing/st\"é\\\x01-\xfd\xfe/x/staging", "'utf-8' codec can't encode characters in position 55-56: surrogates not allowed"},
		{"/missing//./a\xe2\x82/staging", "'utf-8' codec can't encode characters in position 42-43: surrogates not allowed"},
		{"/missing/state-\xfd/staging", "'utf-8' codec can't encode character '\\udcfd' in position 47: surrogates not allowed"},
	} {
		failed := unencodableStaging(stagingStore{staging: test.staging})
		if failed == nil || failed.Code != CodeInvalidState || failed.Message != test.want || failed.Path != "" {
			t.Errorf("%q: %+v, want invalid_state %q", test.staging, failed, test.want)
		}
	}
	mustSucceed(t, unencodableStaging(stagingStore{staging: "/missing/é/staging"}))
	mustSucceed(t, unencodableStaging(fakeStore{}))
}
