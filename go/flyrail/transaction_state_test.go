//go:build darwin || linux

package flyrail

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"slices"
	"sync"
	"syscall"
	"testing"
	"time"

	"github.com/google/go-cmp/cmp"

	"github.com/nielsmadan/flyrail/go/flyrail/store"
	"github.com/nielsmadan/flyrail/go/internal/seams"
)

type recordingStore struct {
	store.Store
	mu    *sync.Mutex
	names *[]string
}

func (s recordingStore) Lock(ctx context.Context, name string, timeout time.Duration) (store.Unlocker, error) {
	s.mu.Lock()
	*s.names = append(*s.names, name)
	s.mu.Unlock()
	return s.Store.Lock(ctx, name, timeout)
}

type strictStore struct{ store.Store }

func (s strictStore) Read(ctx context.Context, key string) ([]byte, bool, error) {
	if err := ctx.Err(); err != nil {
		return nil, false, err
	}
	return s.Store.Read(ctx, key)
}

func (s strictStore) Write(ctx context.Context, key string, value []byte, expected store.Expected) (bool, error) {
	if err := ctx.Err(); err != nil {
		return false, err
	}
	return s.Store.Write(ctx, key, value, expected)
}

func (s strictStore) Delete(ctx context.Context, key string, expected []byte) (bool, error) {
	if err := ctx.Err(); err != nil {
		return false, err
	}
	return s.Store.Delete(ctx, key, expected)
}

func (s strictStore) Keys(ctx context.Context, prefix string) ([]string, error) {
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	return s.Store.Keys(ctx, prefix)
}

type failingStore struct {
	store.Store
	key string
}

func (s failingStore) Write(ctx context.Context, key string, value []byte, expected store.Expected) (bool, error) {
	if key == s.key {
		return false, &store.IOError{Message: "[Errno 5] Input/output error", Errno: syscall.EIO}
	}
	return s.Store.Write(ctx, key, value, expected)
}

func holdLock(t *testing.T, st store.Store, name string) store.Unlocker {
	t.Helper()
	held, err := st.Lock(t.Context(), name, 0)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = held.Unlock() })
	return held
}

func TestTransactionLocksTakeTheIndexFirstThenResourcesInDestinationOrder(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	var names []string
	st := recordingStore{Store: store.NewMemoryStore(), mu: &sync.Mutex{}, names: &names}
	z, a := documentRef(t, filepath.Join(root, "z")), documentRef(t, filepath.Join(root, "a"))
	locks, failed := acquireTransactionLocks(t.Context(), st, testIndexKey, []ResourceRef{z, a}, 0, nil)
	mustSucceed(t, failed)
	t.Cleanup(func() { _ = locks.Unlock() })
	want := []string{testIndexKey, resourceLockName(a.Destination), resourceLockName(z.Destination)}
	if diff := cmp.Diff(want, names); diff != "" {
		t.Errorf("lock order (-want +got):\n%s", diff)
	}
	if got := []ResourceRef{locks.resources[0].reference, locks.resources[1].reference}; !slices.Equal(got, []ResourceRef{a, z}) {
		t.Errorf("resources = %v", got)
	}
	for _, reference := range []ResourceRef{a, z} {
		if !hasRecord(t, st, authorityKey(reference)) {
			t.Errorf("%s was not admitted", reference.Destination)
		}
	}
}

func TestEachTransactionLockWaitsItsOwnFullTimeout(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	st := store.NewMemoryStore()
	a, z := documentRef(t, filepath.Join(root, "a")), documentRef(t, filepath.Join(root, "z"))
	first, second := holdLock(t, st, resourceLockName(a.Destination)), holdLock(t, st, resourceLockName(z.Destination))
	time.AfterFunc(300*time.Millisecond, func() { _ = first.Unlock() })
	time.AfterFunc(600*time.Millisecond, func() { _ = second.Unlock() })
	started := time.Now()
	locks, failed := acquireTransactionLocks(t.Context(), st, testIndexKey, []ResourceRef{z, a}, 400*time.Millisecond, nil)
	mustSucceed(t, failed)
	t.Cleanup(func() { _ = locks.Unlock() })
	if elapsed := time.Since(started); elapsed < 600*time.Millisecond {
		t.Errorf("acquired after %v, before the second lock was released", elapsed)
	}
}

func TestABusyTransactionLockReleasesEverythingItAcquired(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	st := store.NewMemoryStore()
	a, z := documentRef(t, filepath.Join(root, "a")), documentRef(t, filepath.Join(root, "z"))
	holdLock(t, st, resourceLockName(z.Destination))
	started := time.Now()
	_, failed := acquireTransactionLocks(t.Context(), st, testIndexKey, []ResourceRef{a, z}, 50*time.Millisecond, nil)
	wantFailure(t, failed, CodeBusy, "state store lock is busy: "+resourceLockName(z.Destination), "")
	if !errors.Is(failed, store.ErrBusy) {
		t.Errorf("failure %v does not wrap ErrBusy", failed)
	}
	if elapsed := time.Since(started); elapsed < 50*time.Millisecond {
		t.Errorf("gave up after %v", elapsed)
	}
	for _, name := range []string{testIndexKey, resourceLockName(a.Destination)} {
		holdLock(t, st, name)
	}
}

func TestTransactionLocksHonourCancellationAndTheIndexLock(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	st := store.NewMemoryStore()
	a := documentRef(t, filepath.Join(root, "a"))
	index := holdLock(t, st, testIndexKey)
	_, failed := acquireTransactionLocks(t.Context(), st, testIndexKey, []ResourceRef{a}, 0, nil)
	wantFailure(t, failed, CodeBusy, "state store lock is busy: "+testIndexKey, "")
	holdLock(t, st, resourceLockName(a.Destination))
	if err := index.Unlock(); err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithCancel(t.Context())
	time.AfterFunc(30*time.Millisecond, cancel)
	started := time.Now()
	_, failed = acquireTransactionLocks(ctx, st, testIndexKey, []ResourceRef{a}, time.Minute, nil)
	if failed == nil || failed.Code != CodeBusy || !errors.Is(failed, context.Canceled) {
		t.Fatalf("cancelled acquisition = %v", failed)
	}
	if elapsed := time.Since(started); elapsed > 30*time.Second {
		t.Errorf("cancellation took %v", elapsed)
	}
	holdLock(t, st, testIndexKey)
}

func TestFailedIndexValidationReleasesTheIndexLock(t *testing.T) {
	t.Parallel()
	st := store.NewMemoryStore()
	refused := failure(CodeConcurrentChange, "index changed while acquiring locks", "")
	_, failed := acquireTransactionLocks(t.Context(), st, testIndexKey, nil, 0, func() *TargetError { return refused })
	if failed != refused {
		t.Fatalf("acquisition = %v, want the validation failure", failed)
	}
	holdLock(t, st, testIndexKey)
}

func TestTransactionLocksCreateMissingParentsAndRecordThem(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	st := store.NewMemoryStore()
	destination := filepath.Join(root, "new", "nested", "AGENTS.md")
	locks, failed := acquireTransactionLocks(t.Context(), st, testIndexKey, []ResourceRef{documentRef(t, destination)}, 0, nil)
	mustSucceed(t, failed)
	t.Cleanup(func() { _ = locks.Unlock() })
	var created []string
	for _, item := range locks.created {
		created = append(created, item.path)
	}
	if diff := cmp.Diff([]string{filepath.Join(root, "new"), filepath.Join(root, "new", "nested")}, created); diff != "" {
		t.Errorf("created (-want +got):\n%s", diff)
	}
}

func TestCancellationIsHonouredOnlyBeforePreparation(t *testing.T) {
	t.Parallel()
	for _, boundary := range []string{seams.ResourcePrecondition, seams.Preparation, seams.Staged, seams.ReceiptCommitted} {
		t.Run(boundary, func(t *testing.T) {
			t.Parallel()
			st := strictStore{store.NewMemoryStore()}
			reference := admitted(t, st, documentRef(t, filepath.Join(tempRoot(t), "AGENTS.md")))
			plan := testPlan(t, st, reference, revision{}, fileRevision(t, "generated\n"), nil, testReceipt(t, reference, '1'))
			setup := newTransaction(t, st, plan)
			ctx, cancel := context.WithCancel(t.Context())
			hooks := &seams.Hooks{Observe: func(event seams.Event) {
				if event.Kind == seams.KindBoundary && event.Boundary == boundary {
					cancel()
				}
			}}
			outcome := setup.execute(seams.With(ctx, hooks), t)
			if boundary == seams.ResourcePrecondition {
				if outcome.err == nil || !errors.Is(outcome.err, context.Canceled) || len(outcome.completed) > 0 {
					t.Fatalf("outcome = %+v, want a cancelled failure", outcome)
				}
				if exists(t, reference.Destination) {
					t.Error("a cancelled transaction published")
				}
				return
			}
			if outcome.err != nil || len(outcome.completed) != 1 {
				t.Fatalf("outcome = %+v, want the prepared transaction to finish", outcome)
			}
			if got := setup.storedIndex(t); got == nil || !sameRecord(*got, setup.completed) {
				t.Errorf("index = %+v, want completed", got)
			}
			assertSettled(t, st, reference)
		})
	}
}

func TestExecuteRefusesInconsistentArguments(t *testing.T) {
	t.Parallel()
	st := store.NewMemoryStore()
	root := tempRoot(t)
	reference := admitted(t, st, documentRef(t, filepath.Join(root, "AGENTS.md")))
	other := admitted(t, st, documentRef(t, filepath.Join(root, "other")))
	plan := testPlan(t, st, reference, revision{}, fileRevision(t, "x\n"), nil, testReceipt(t, reference, '1'))
	setup := newTransaction(t, st, plan)
	wrongCompletion := must(completedIndex("team", testContextDigest, &generation{version: "1", bundleDigest: testContextDigest, renderDigest: testContextDigest}))
	_, failed := executeTransaction(t.Context(), setup.locks, setup.plans, nil, setup.pending, wrongCompletion)
	wantFailure(t, failed, CodeInvalidState, "installation index completion disagrees with preparation", "")
	otherPlan := testPlan(t, st, other, revision{}, fileRevision(t, "x\n"), nil, testReceipt(t, other, '2'))
	_, failed = executeTransaction(t.Context(), setup.locks, []ResourcePlan{otherPlan}, nil, setup.pending, setup.completed)
	wantFailure(t, failed, CodeConcurrentChange, "resource locks disagree", "")
	refused := failure(CodeConflict, "claim overlaps recorded ownership", reference.Destination)
	outcome, failed := executeTransaction(t.Context(), setup.locks, []ResourcePlan{plan.withError(refused)}, nil, setup.pending, setup.completed)
	mustSucceed(t, failed)
	wantFailure(t, outcome.err, CodeConflict, "claim overlaps recorded ownership", reference.Destination)
	recovering := plan
	recovering.recoveryPaths = []string{plan.transactionDirectory}
	outcome, failed = executeTransaction(t.Context(), setup.locks, []ResourcePlan{recovering}, nil, setup.pending, setup.completed)
	mustSucceed(t, failed)
	wantFailure(t, outcome.err, CodeRecoveryNeeded, "preview requires recovery and re-observation", "")
	if setup.storedIndex(t) != nil || exists(t, reference.Destination) {
		t.Error("a refused transaction changed state")
	}
}

func TestUnchangedResourcesAreNotJournaled(t *testing.T) {
	t.Parallel()
	st := store.NewMemoryStore()
	destination := filepath.Join(tempRoot(t), "AGENTS.md")
	exclusiveWrite(t, destination, "same\n", 0o600)
	reference := admitted(t, st, documentRef(t, destination))
	old := testReceipt(t, reference, '0')
	putReceipt(t, st, reference, old)
	before := mustObserve(t, destination)
	setup := newTransaction(t, st, testPlan(t, st, reference, before, before, &old, old))
	journaled := false
	hooks := &seams.Hooks{Observe: func(event seams.Event) {
		journaled = journaled || event.Kind == seams.KindBoundary && event.Boundary == seams.Preparation
	}}
	outcome := setup.execute(seams.With(t.Context(), hooks), t)
	if diff := cmp.Diff([]ResourceRef{reference}, outcome.unchanged); diff != "" || outcome.err != nil || len(outcome.completed) > 0 {
		t.Fatalf("outcome = %+v", outcome)
	}
	if journaled {
		t.Error("an unchanged resource was journaled")
	}
	if got := setup.storedIndex(t); got == nil || !sameRecord(*got, setup.completed) {
		t.Errorf("index = %+v", got)
	}
}

func TestAdoptionJournalsOwnershipWithoutMovingContent(t *testing.T) {
	t.Parallel()
	st := store.NewMemoryStore()
	_, plan := shapedPlan(t, st, tempRoot(t), shape{name: "adopt", adopt: true})
	reference := plan.resource
	setup := newTransaction(t, st, plan)
	var journaled bool
	var renames int
	hooks := &seams.Hooks{Observe: func(event seams.Event) {
		if event.Kind == seams.KindRename {
			renames++
		}
		if event.Kind == seams.KindBoundary && event.Boundary == seams.Receipt {
			journaled = hasRecord(t, st, journalKey(reference))
		}
	}}
	outcome := setup.execute(seams.With(t.Context(), hooks), t)
	if outcome.err != nil || len(outcome.completed) != 1 {
		t.Fatalf("outcome = %+v", outcome)
	}
	if !journaled || renames != 0 {
		t.Errorf("journaled %v with %d renames, want a journal and no content move", journaled, renames)
	}
	if !mustObserve(t, reference.Destination).equal(plan.before) {
		t.Error("adoption changed the destination")
	}
	if got := receiptOf(t, st, reference); got == nil || !sameRecord(*got, plan.receipt) {
		t.Errorf("receipt = %+v", got)
	}
	assertSettled(t, st, reference)
}

func TestTakeoverReplacesForeignContentAndRemovesTheBackup(t *testing.T) {
	t.Parallel()
	st := store.NewMemoryStore()
	destination := filepath.Join(tempRoot(t), "AGENTS.md")
	exclusiveWrite(t, destination, "foreign\n", 0o644)
	reference := admitted(t, st, documentRef(t, destination))
	plan := testPlan(t, st, reference, mustObserve(t, destination), fileRevision(t, "mine\n"), nil, testReceipt(t, reference, '1'))
	outcome := newTransaction(t, st, plan).execute(t.Context(), t)
	if outcome.err != nil {
		t.Fatal(outcome.err)
	}
	if got := readText(t, destination); got != "mine\n" {
		t.Errorf("destination = %q", got)
	}
	assertSettled(t, st, reference)
}

func retargeting(t *testing.T, st store.Store, root string) (old, moved ResourceRef, plans []ResourcePlan) {
	t.Helper()
	old = admitted(t, st, documentRef(t, filepath.Join(root, "a-old.md")))
	moved = admitted(t, st, documentRef(t, filepath.Join(root, "b-new.md")))
	exclusiveWrite(t, old.Destination, "content\n", 0o600)
	recorded := testReceipt(t, old, '0')
	putReceipt(t, st, old, recorded)
	plans = []ResourcePlan{
		testPlan(t, st, old, mustObserve(t, old.Destination), revision{}, &recorded, testReceipt(t, old, '1')),
		testPlan(t, st, moved, revision{}, fileRevision(t, "content\n"), nil, testReceipt(t, moved, '2')),
	}
	return old, moved, plans
}

func TestRetargetingRetiresTheOldDestinationAndCreatesTheNewOne(t *testing.T) {
	t.Parallel()
	st := store.NewMemoryStore()
	old, moved, plans := retargeting(t, st, tempRoot(t))
	outcome := newTransaction(t, st, plans...).execute(t.Context(), t)
	if diff := cmp.Diff([]ResourceRef{old, moved}, outcome.completed); diff != "" || outcome.err != nil {
		t.Fatalf("outcome = %+v", outcome)
	}
	if exists(t, old.Destination) || readText(t, moved.Destination) != "content\n" {
		t.Error("the content was not retargeted")
	}
	for index, reference := range []ResourceRef{old, moved} {
		if got := receiptOf(t, st, reference); got == nil || !sameRecord(*got, plans[index].receipt) {
			t.Errorf("%s receipt = %+v", reference.Destination, got)
		}
		assertSettled(t, st, reference)
	}
}

func TestAPartialCommitKeepsEarlierResourcesAndARetryCompletes(t *testing.T) {
	t.Parallel()
	st := store.NewMemoryStore()
	old, moved, plans := retargeting(t, st, tempRoot(t))
	setup := newTransaction(t, st, plans...)
	outcome := setup.execute(seams.With(t.Context(), faultAfter(seams.PublishReceipt, 1)), t)
	if diff := cmp.Diff([]ResourceRef{old}, outcome.completed); diff != "" || outcome.failed == nil || *outcome.failed != moved {
		t.Fatalf("outcome = %+v", outcome)
	}
	wantFailure(t, outcome.err, CodeIOError, "injected publish-receipt fault", "")
	if exists(t, old.Destination) || exists(t, moved.Destination) {
		t.Error("the partial commit left the wrong destinations")
	}
	if got := setup.storedIndex(t); got == nil || !sameRecord(*got, setup.pending) {
		t.Errorf("index = %+v, want pending", got)
	}
	assertSettled(t, st, moved)
	setup.finish(t)
	retry := newTransaction(t, st, testPlan(t, st, moved, revision{}, fileRevision(t, "content\n"), nil, testReceipt(t, moved, '3')))
	if outcome := retry.execute(t.Context(), t); outcome.err != nil {
		t.Fatal(outcome.err)
	}
	if got := readText(t, moved.Destination); got != "content\n" {
		t.Errorf("retried destination = %q", got)
	}
}

func TestIndexWriteFailuresStopBeforeOrBetweenResources(t *testing.T) {
	t.Parallel()
	t.Run("pending index changed", func(t *testing.T) {
		t.Parallel()
		st := store.NewMemoryStore()
		_, _, plans := twoDocuments(t, st, tempRoot(t))
		setup := newTransaction(t, st, plans...)
		mustSucceed(t, writeRecord(t.Context(), st, testIndexKey, setup.completed, nil))
		_, failed := executeTransaction(t.Context(), setup.locks, setup.plans, nil, setup.pending, setup.completed)
		wantFailure(t, failed, CodeConcurrentChange, testIndexKey+": state changed", "")
		for _, plan := range plans {
			if exists(t, plan.resource.Destination) || hasRecord(t, st, preparationKey(plan.resource)) {
				t.Errorf("%s was touched", plan.resource.Destination)
			}
		}
	})
	t.Run("pending index unwritable", func(t *testing.T) {
		t.Parallel()
		st := failingStore{Store: store.NewMemoryStore(), key: testIndexKey}
		_, _, plans := twoDocuments(t, st, tempRoot(t))
		_, failed := executeTransaction(t.Context(), newTransaction(t, st, plans...).locks, plans, nil, must(pendingIndex("team", testContextDigest, nil, nil, []ResourceRef{plans[0].resource, plans[1].resource})), must(completedIndex("team", testContextDigest, nil)))
		if failed == nil || failed.Code != CodeIOError || failed.Errno != syscall.EIO {
			t.Fatalf("failure = %v, want EIO", failed)
		}
	})
	t.Run("index changed between resources", func(t *testing.T) {
		t.Parallel()
		st := store.NewMemoryStore()
		first, second, plans := twoDocuments(t, st, tempRoot(t))
		setup := newTransaction(t, st, plans...)
		requires := 0
		hooks := &seams.Hooks{Observe: func(event seams.Event) {
			if event.Kind == seams.KindBoundary && event.Boundary == seams.IndexRequire {
				if requires++; requires == 2 {
					mustSucceed(t, writeRecord(t.Context(), st, testIndexKey, setup.completed, &setup.pending))
				}
			}
		}}
		outcome := setup.execute(seams.With(t.Context(), hooks), t)
		if diff := cmp.Diff([]ResourceRef{first}, outcome.completed); diff != "" || outcome.failed == nil || *outcome.failed != second {
			t.Fatalf("outcome = %+v", outcome)
		}
		wantFailure(t, outcome.err, CodeConcurrentChange, "installation index changed", "")
		assertSettled(t, st, second)
	})
	t.Run("completion refused", func(t *testing.T) {
		t.Parallel()
		st := store.NewMemoryStore()
		first, second, plans := twoDocuments(t, st, tempRoot(t))
		setup := newTransaction(t, st, plans...)
		hooks := &seams.Hooks{Observe: func(event seams.Event) {
			if event.Kind == seams.KindBoundary && event.Boundary == seams.IndexCompletion {
				mustSucceed(t, writeRecord(t.Context(), st, testIndexKey, setup.completed, &setup.pending))
			}
		}}
		outcome := setup.execute(seams.With(t.Context(), hooks), t)
		if diff := cmp.Diff([]ResourceRef{first, second}, outcome.completed); diff != "" || outcome.failed != nil {
			t.Fatalf("outcome = %+v", outcome)
		}
		wantFailure(t, outcome.err, CodeConcurrentChange, testIndexKey+": state changed", "")
	})
}

func TestAFailedRollbackIsRetriedByTheNextRecovery(t *testing.T) {
	t.Parallel()
	st := store.NewMemoryStore()
	_, plan := shapedPlan(t, st, tempRoot(t), transactionShapes[0])
	setup := newTransaction(t, st, plan)
	outcome := setup.execute(seams.With(t.Context(), faultAt(seams.PublishReceipt, seams.Rollback)), t)
	wantFailure(t, outcome.err, CodeIOError, "injected publish-receipt fault; recovery: injected rollback fault", "")
	if !hasRecord(t, st, journalKey(plan.resource)) {
		t.Fatal("the failed rollback did not retain the journal")
	}
	mustSucceed(t, recoverResource(t.Context(), st, setup.locks.resources[0]))
	assertConverged(t, st, plan, false)
}

func preparationOnly(t *testing.T, st store.Store, reference ResourceRef, directory string) preparation {
	t.Helper()
	prepared := must(newPreparation(reference, testTransaction, directory, stateSchemaVersion))
	mustSucceed(t, writeRecord(t.Context(), st, preparationKey(reference), prepared, nil))
	return prepared
}

func TestRecoveryReleasesOrRetainsJournalLessPreparations(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	setupCase := func(t *testing.T) (store.Store, ResourceRef, *resourceLock, string) {
		t.Helper()
		st := store.NewMemoryStore()
		reference := admitted(t, st, documentRef(t, filepath.Join(tempRoot(t), "AGENTS.md")))
		directory := must(TransactionPath(reference.Destination, testTransaction))
		return st, reference, lockOne(t, st, reference), directory
	}
	t.Run("missing unrecognized directory", func(t *testing.T) {
		t.Parallel()
		st, reference, guard, _ := setupCase(t)
		preparationOnly(t, st, reference, filepath.Join(root, "moved-store", "txn"))
		mustSucceed(t, recoverResource(t.Context(), st, guard))
		if hasRecord(t, st, preparationKey(reference)) {
			t.Error("the preparation was retained")
		}
	})
	t.Run("existing unrecognized directory", func(t *testing.T) {
		t.Parallel()
		st, reference, guard, _ := setupCase(t)
		elsewhere := filepath.Join(tempRoot(t), "elsewhere")
		mkdir(t, elsewhere)
		preparationOnly(t, st, reference, elsewhere)
		wantFailure(t, recoverResource(t.Context(), st, guard), CodeRecoveryNeeded, "transaction record names an unrecognized directory", "")
		if !hasRecord(t, st, preparationKey(reference)) || !exists(t, elsewhere) {
			t.Error("the evidence was not retained")
		}
	})
	t.Run("recognized partial staging", func(t *testing.T) {
		t.Parallel()
		st, reference, guard, directory := setupCase(t)
		preparationOnly(t, st, reference, directory)
		if err := os.Mkdir(directory, 0o700); err != nil {
			t.Fatal(err)
		}
		if err := os.Mkdir(filepath.Join(directory, "staging"), 0o700); err != nil {
			t.Fatal(err)
		}
		_, failed := writeRevision(filepath.Join(directory, "staging", "resource"), fileRevision(t, "partial\n"))
		mustSucceed(t, failed)
		mustSucceed(t, recoverResource(t.Context(), st, guard))
		if hasRecord(t, st, preparationKey(reference)) || exists(t, directory) {
			t.Error("the abandoned transaction was not removed")
		}
	})
	for _, unknown := range []string{"other", "backup/resource"} {
		t.Run("unknown "+unknown, func(t *testing.T) {
			t.Parallel()
			st, reference, guard, directory := setupCase(t)
			preparationOnly(t, st, reference, directory)
			if err := os.Mkdir(directory, 0o700); err != nil {
				t.Fatal(err)
			}
			if err := os.MkdirAll(filepath.Dir(filepath.Join(directory, unknown)), 0o700); err != nil {
				t.Fatal(err)
			}
			exclusiveWrite(t, filepath.Join(directory, unknown), "evidence\n", 0o600)
			wantFailure(t, recoverResource(t.Context(), st, guard), CodeRecoveryNeeded, "unrecognized transaction data retained", directory)
			if !hasRecord(t, st, preparationKey(reference)) || !exists(t, filepath.Join(directory, unknown)) {
				t.Error("the evidence was not retained")
			}
		})
	}
}

func TestRecoveryRefusesUnmatchedTransactionRecords(t *testing.T) {
	t.Parallel()
	t.Run("journal without preparation", func(t *testing.T) {
		t.Parallel()
		c := preparedReplacement(t, store.NewMemoryStore(), filepath.Join(tempRoot(t), "AGENTS.md"))
		prepared, ok, failed := readRecord(t.Context(), c.st, preparationKey(c.reference), "Preparation", preparationFromState)
		mustSucceed(t, failed)
		if !ok {
			t.Fatal("no preparation")
		}
		mustSucceed(t, deleteRecord(t.Context(), c.st, preparationKey(c.reference), prepared))
		wantFailure(t, recoverResource(t.Context(), c.st, c.guard), CodeInvalidState, "transaction record has no matching preparation", "")
	})
	t.Run("foreign receipt", func(t *testing.T) {
		t.Parallel()
		c := preparedReplacement(t, store.NewMemoryStore(), filepath.Join(tempRoot(t), "AGENTS.md"))
		mustSucceed(t, writeRecord(t.Context(), c.st, receiptKey(c.reference), testReceipt(t, c.reference, '9'), &c.old))
		wantFailure(t, recoverResource(t.Context(), c.st, c.guard), CodeRecoveryNeeded, "receipt disagrees with transaction", c.reference.Destination)
		if !hasRecord(t, c.st, journalKey(c.reference)) {
			t.Error("the journal was not retained")
		}
	})
	t.Run("changed journal before commit", func(t *testing.T) {
		t.Parallel()
		c := preparedReplacement(t, store.NewMemoryStore(), filepath.Join(tempRoot(t), "AGENTS.md"))
		changed := c.prepared
		changed.ancestors = nil
		mustSucceed(t, writeRecord(t.Context(), c.st, journalKey(c.reference), changed, &c.prepared))
		_, failed := publish(t.Context(), c.st, c.prepared)
		wantFailure(t, failed, CodeRecoveryNeeded, "transaction record changed", c.plan.transactionDirectory)
	})
}

func TestPrepareRefusesStalePlansAndRetainedRecords(t *testing.T) {
	t.Parallel()
	fresh := func(t *testing.T) (store.Store, ResourceRef, ResourcePlan, *resourceLock) {
		t.Helper()
		st := store.NewMemoryStore()
		destination := filepath.Join(tempRoot(t), "AGENTS.md")
		exclusiveWrite(t, destination, "before\n", 0o600)
		reference := admitted(t, st, documentRef(t, destination))
		plan := testPlan(t, st, reference, mustObserve(t, destination), fileRevision(t, "after\n"), nil, testReceipt(t, reference, '1'))
		return st, reference, plan, lockOne(t, st, reference)
	}
	t.Run("changed destination", func(t *testing.T) {
		t.Parallel()
		st, reference, plan, guard := fresh(t)
		if err := os.WriteFile(reference.Destination, []byte("edited\n"), 0o600); err != nil {
			t.Fatal(err)
		}
		_, failed := prepare(t.Context(), st, plan, guard)
		wantFailure(t, failed, CodeConcurrentChange, "complete resource revision changed", reference.Destination)
	})
	t.Run("changed receipt", func(t *testing.T) {
		t.Parallel()
		st, reference, plan, guard := fresh(t)
		putReceipt(t, st, reference, testReceipt(t, reference, '0'))
		_, failed := prepare(t.Context(), st, plan, guard)
		wantFailure(t, failed, CodeConcurrentChange, "receipt changed during preparation", reference.Destination)
	})
	t.Run("required resource disappeared", func(t *testing.T) {
		t.Parallel()
		st, reference, plan, guard := fresh(t)
		plan.before, plan.requireExisting = revision{}, true
		if err := os.Remove(reference.Destination); err != nil {
			t.Fatal(err)
		}
		_, failed := prepare(t.Context(), st, plan, guard)
		wantFailure(t, failed, CodeConcurrentChange, "required existing resource disappeared", reference.Destination)
	})
	t.Run("changed ancestor", func(t *testing.T) {
		t.Parallel()
		st, reference, plan, guard := fresh(t)
		if err := os.Chmod(filepath.Dir(reference.Destination), 0o750); err != nil {
			t.Fatal(err)
		}
		_, failed := prepare(t.Context(), st, plan, guard)
		wantFailure(t, failed, CodeConcurrentChange, "resource ancestor changed", reference.Destination)
	})
	t.Run("unrecognized directory", func(t *testing.T) {
		t.Parallel()
		st, _, plan, guard := fresh(t)
		plan.transactionDirectory = filepath.Join(tempRoot(t), "txn")
		_, failed := prepare(t.Context(), st, plan, guard)
		wantFailure(t, failed, CodeInvalidState, "planned transaction directory is not recognized", "")
	})
	t.Run("another resource's lock", func(t *testing.T) {
		t.Parallel()
		st, reference, plan, _ := fresh(t)
		other := lockOne(t, st, documentRef(t, filepath.Join(filepath.Dir(reference.Destination), "other")))
		_, failed := prepare(t.Context(), st, plan, other)
		wantFailure(t, failed, CodeConcurrentChange, "resource lock disagrees", reference.Destination)
	})
	t.Run("retained preparation", func(t *testing.T) {
		t.Parallel()
		st, reference, plan, guard := fresh(t)
		directory := must(TransactionPath(reference.Destination, testTransaction))
		preparationOnly(t, st, reference, directory)
		if err := os.Mkdir(directory, 0o700); err != nil {
			t.Fatal(err)
		}
		_, failed := prepare(t.Context(), st, plan, guard)
		wantFailure(t, failed, CodeRecoveryNeeded, "transaction record retained", directory)
	})
	t.Run("existing transaction directory", func(t *testing.T) {
		t.Parallel()
		st, reference, plan, guard := fresh(t)
		hooks := &seams.Hooks{Observe: func(event seams.Event) {
			if event.Kind == seams.KindBoundary && event.Boundary == seams.Preparation {
				if err := os.Mkdir(plan.transactionDirectory, 0o700); err != nil {
					t.Fatal(err)
				}
			}
		}}
		_, failed := prepare(seams.With(t.Context(), hooks), st, plan, guard)
		wantFailure(t, failed, CodeRecoveryNeeded, "unrecognized transaction data retained", plan.transactionDirectory)
		if hasRecord(t, st, preparationKey(reference)) {
			t.Error("the preparation was not released")
		}
	})
}
