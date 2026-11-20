package flyrail

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"io/fs"
	"slices"
	"syscall"
	"time"
	"unicode/utf8"

	"github.com/nielsmadan/flyrail/go/flyrail/store"
	"github.com/nielsmadan/flyrail/go/internal/jsonx"
	"github.com/nielsmadan/flyrail/go/internal/native"
	"github.com/nielsmadan/flyrail/go/internal/pathx"
	"github.com/nielsmadan/flyrail/go/internal/seams"
	"github.com/nielsmadan/flyrail/go/internal/sysx"
)

func resourceLockName(destination string) string {
	return "resource/" + destinationKey(destination)
}

type resourceLock struct {
	reference ResourceRef
	held      store.Unlocker
}

func acquireResourceLock(ctx context.Context, st store.Store, reference ResourceRef, timeout time.Duration) (*resourceLock, *TargetError) {
	held, failed := storeLock(ctx, st, resourceLockName(reference.Destination), timeout)
	if failed != nil {
		return nil, failed
	}
	return &resourceLock{reference: reference, held: held}, nil
}

// Unlock releases the resource lock; it is idempotent.
func (l *resourceLock) Unlock() error {
	return l.held.Unlock()
}

func unlockResources(locks []*resourceLock) error {
	var failures []error
	for _, held := range slices.Backward(locks) {
		failures = append(failures, held.Unlock())
	}
	return errors.Join(failures...)
}

func lockResources(ctx context.Context, st store.Store, references []ResourceRef, timeout time.Duration) ([]*resourceLock, *TargetError) {
	var acquired []*resourceLock
	for _, reference := range destinationOrder(references) {
		held, failed := acquireResourceLock(ctx, st, reference, timeout)
		if failed != nil {
			_ = unlockResources(acquired)
			return nil, failed
		}
		acquired = append(acquired, held)
	}
	return acquired, nil
}

type transactionLocks struct {
	store     store.Store
	indexKey  string
	index     store.Unlocker
	resources []*resourceLock
	created   []ancestor
}

const preparationDirectoryPrefix = `{"fields":{"directory":{"path":`

func unencodableStaging(st store.Store) *TargetError {
	staging, ok := st.StagingDirectory()
	if !ok {
		return nil
	}
	if physical, err := native.PhysicalLocation(staging); err == nil {
		staging = physical
	}
	if utf8.ValidString(staging) {
		return nil
	}
	path := normalizedPosixPath(staging)
	invalid := func(at int) bool {
		character, size := utf8.DecodeRuneInString(path[at:])
		return character == utf8.RuneError && size == 1
	}
	start := 0
	for !invalid(start) {
		_, size := utf8.DecodeRuneInString(path[start:])
		start += size
	}
	end := start + 1
	for end < len(path) && invalid(end) {
		end++
	}
	position := utf8.RuneCountInString(preparationDirectoryPrefix) + utf8.RuneCount(jsonx.AppendEscaped(nil, path[:start])) - 1
	message := fmt.Sprintf("'utf-8' codec can't encode characters in position %d-%d: surrogates not allowed", position, position+end-start-1)
	if end-start == 1 {
		message = fmt.Sprintf("'utf-8' codec can't encode character '\\udc%02x' in position %d: surrogates not allowed", path[start], position)
	}
	return failure(CodeInvalidState, message, "")
}

// Locks the index, then each resource in destination order; every lock gets the full timeout.
func acquireTransactionLocks(ctx context.Context, st store.Store, indexKey string, references []ResourceRef, timeout time.Duration, validate func() *TargetError) (*transactionLocks, *TargetError) {
	if failed := unencodableStaging(st); failed != nil {
		return nil, failed
	}
	created, failed := createStoreParents(st, nil)
	if failed != nil {
		return nil, failed
	}
	index, failed := storeLock(ctx, st, indexKey, timeout)
	if failed != nil {
		return nil, failed
	}
	locks := &transactionLocks{store: st, indexKey: indexKey, index: index, created: created}
	if validate != nil {
		if failed := validate(); failed != nil {
			_ = locks.Unlock()
			return nil, failed
		}
	}
	for _, reference := range destinationOrder(references) {
		if failed := locks.addResource(ctx, reference, timeout); failed != nil {
			_ = locks.Unlock()
			return nil, failed
		}
	}
	return locks, nil
}

func (l *transactionLocks) addResource(ctx context.Context, reference ResourceRef, timeout time.Duration) *TargetError {
	if failed := validateKind(reference); failed != nil {
		return failed
	}
	held, failed := acquireResourceLock(ctx, l.store, reference, timeout)
	if failed != nil {
		return failed
	}
	l.resources = append(l.resources, held)
	if l.created, failed = createParents(reference.Destination, l.created); failed != nil {
		return failed
	}
	return admit(ctx, l.store, reference)
}

// Unlock releases the resource locks in reverse order and then the index lock; it is idempotent.
func (l *transactionLocks) Unlock() error {
	return errors.Join(unlockResources(l.resources), l.index.Unlock())
}

func (l *transactionLocks) lockFor(reference ResourceRef) *resourceLock {
	for _, held := range l.resources {
		if resourceRefsEqual(held.reference, reference) {
			return held
		}
	}
	return nil
}

type transactionOutcome struct {
	completed []ResourceRef
	unchanged []ResourceRef
	failed    *ResourceRef
	err       *TargetError
}

func sameRecord(a, b stateRecord) bool {
	return bytes.Equal(encodeRecord(a), encodeRecord(b))
}

func sameOptionalReceipt(a, b *receipt) bool {
	if a == nil || b == nil {
		return a == b
	}
	return sameRecord(*a, *b)
}

func inject(ctx context.Context, boundary string) *TargetError {
	if seams.Inject(ctx, boundary) {
		return newTargetError(CodeIOError, "injected "+boundary+" fault", "", syscall.EIO, nil)
	}
	err := seams.Failure(ctx, boundary)
	var timedOut seams.BarrierTimedOut
	var pathErr *fs.PathError
	switch {
	case errors.As(err, &timedOut):
		return newTargetError(CodeIOError, err.Error(), timedOut.Path, 0, nil)
	case errors.As(err, &pathErr):
		return ioFailure(err, pathErr.Path)
	case err != nil:
		return newTargetError(CodeIOError, err.Error(), "", 0, nil)
	}
	return nil
}

func requireIndex(ctx context.Context, st store.Store, key string, expected *index) *TargetError {
	data, present, err := st.Read(ctx, key)
	if err != nil {
		return storeFailure(err)
	}
	if present != (expected != nil) || expected != nil && !bytes.Equal(data, encodeRecord(*expected)) {
		return stateFailure(st, CodeConcurrentChange, "installation index changed")
	}
	return nil
}

func executeTransaction(ctx context.Context, locks *transactionLocks, plans []ResourcePlan, previous *index, pending, completed index, inputs ...loadoutInput) (transactionOutcome, *TargetError) {
	st := locks.store
	expected, err := newIndex(pending.bundleID, pending.contextDigest, pending.pending, nil, nil, nil, stateSchemaVersion)
	if err != nil {
		return transactionOutcome{}, stateFailure(st, CodeInvalidState, err.Error())
	}
	if !sameRecord(completed, expected) {
		return transactionOutcome{}, stateFailure(st, CodeInvalidState, "installation index completion disagrees with preparation")
	}
	if len(plans) != len(locks.resources) || slices.ContainsFunc(plans, func(plan ResourcePlan) bool { return locks.lockFor(plan.resource) == nil }) {
		return transactionOutcome{}, stateFailure(st, CodeConcurrentChange, "resource locks disagree")
	}
	for _, plan := range plans {
		if plan.err != nil {
			return transactionOutcome{err: plan.Err()}, nil
		}
	}
	if slices.ContainsFunc(plans, func(plan ResourcePlan) bool { return len(plan.recoveryPaths) > 0 }) {
		return transactionOutcome{err: failure(CodeRecoveryNeeded, "preview requires recovery and re-observation", "")}, nil
	}
	if failed := writeRecord(ctx, st, locks.indexKey, pending, previous); failed != nil {
		return transactionOutcome{}, failed
	}
	var applied, unchanged []ResourceRef
	changed := make(map[string]revision)
	checkRoute := func() *TargetError {
		for _, input := range inputs {
			if revision, found := changed[input.resolved]; found {
				if failed := input.requireBinding(); failed != nil {
					return failed
				}
				if failed := require(input.resolved, revision); failed != nil {
					return failed
				}
			} else {
				if failed := input.require(); failed != nil {
					return failed
				}
			}
		}
		return nil
	}
	for _, plan := range plans {
		if plan.previous != nil && sameRecord(*plan.previous, plan.receipt) && plan.before.equal(plan.after) {
			unchanged = append(unchanged, plan.resource)
			continue
		}
		attempt := applyResource(ctx, locks, plan, pending, checkRoute)
		ctx = context.WithoutCancel(ctx)
		if attempt.err == nil {
			applied = append(applied, plan.resource)
			if len(inputs) != 0 {
				changed[plan.resource.Destination] = attempt.journal.published
			}
			continue
		}
		committed := attempt.committed
		if !committed && attempt.journal != nil {
			current, failed := currentReceipt(ctx, st, *attempt.journal)
			committed = failed == nil && current != nil && sameRecord(*current, attempt.journal.receipt)
		}
		final := attempt.err
		if attempt.guard != nil {
			if recovery := recoverResource(ctx, st, attempt.guard); recovery != nil {
				final = newTargetError(final.Code, final.Message+"; recovery: "+recovery.Message, final.Path, final.Errno, final.cause)
			}
		}
		if committed {
			applied = append(applied, plan.resource)
		}
		failedResource := plan.resource
		return transactionOutcome{completed: applied, unchanged: unchanged, failed: &failedResource, err: final}, nil
	}
	failed := inject(ctx, seams.IndexCompletion)
	if failed == nil {
		failed = writeRecord(ctx, st, locks.indexKey, completed, &pending)
	}
	return transactionOutcome{completed: applied, unchanged: unchanged, err: failed}, nil
}

type resourceAttempt struct {
	journal   *journal
	guard     *resourceLock
	committed bool
	err       *TargetError
}

func applyResource(ctx context.Context, locks *transactionLocks, plan ResourcePlan, pending index, checkRoute func() *TargetError) resourceAttempt {
	st := locks.store
	for _, boundary := range []string{seams.ResourcePrecondition, seams.IndexRequire} {
		if failed := inject(ctx, boundary); failed != nil {
			return resourceAttempt{err: failed}
		}
	}
	if failed := requireIndex(ctx, st, locks.indexKey, &pending); failed != nil {
		return resourceAttempt{err: failed}
	}
	if failed := inject(ctx, seams.GuardLookup); failed != nil {
		return resourceAttempt{err: failed}
	}
	guard := locks.lockFor(plan.resource)
	if guard == nil {
		return resourceAttempt{err: failure(CodeConcurrentChange, "resource lock disagrees", plan.resource.Destination)}
	}
	prepared, failed := prepare(ctx, st, plan, guard)
	if failed != nil {
		return resourceAttempt{guard: guard, err: failed}
	}
	ctx = context.WithoutCancel(ctx)
	attempt := resourceAttempt{journal: &prepared, guard: guard}
	if attempt.err = requireIndex(ctx, st, locks.indexKey, &pending); attempt.err != nil {
		return attempt
	}
	if attempt.err = checkRoute(); attempt.err != nil {
		return attempt
	}
	if attempt.committed, attempt.err = publish(ctx, st, prepared); attempt.err != nil {
		return attempt
	}
	attempt.err = cleanup(ctx, st, prepared)
	return attempt
}

func transactionPaths(directory string) (staged, backup string) {
	return pathx.Join(directory, "staging", "resource"), pathx.Join(directory, "backup", "resource")
}

func invalidRecord(err error, path string) *TargetError {
	return failure(CodeInvalidState, err.Error(), path)
}

// Ignores cancellation once the preparation exists.
func prepare(ctx context.Context, st store.Store, plan ResourcePlan, guard *resourceLock) (journal, *TargetError) {
	reference := plan.resource
	destination := reference.Destination
	if !resourceRefsEqual(guard.reference, reference) {
		return journal{}, failure(CodeConcurrentChange, "resource lock disagrees", destination)
	}
	directory := plan.transactionDirectory
	if !recognizedTransactionDirectory(st, destination, plan.receipt.transactionID, directory) {
		return journal{}, failure(CodeInvalidState, "planned transaction directory is not recognized", "")
	}
	current, failed := resourceAncestors(destination, pathx.Join(directory, "staging"))
	if failed != nil {
		return journal{}, failed
	}
	if !ancestorsEqual(current, plan.ancestors) {
		return journal{}, failure(CodeConcurrentChange, "resource ancestor changed", destination)
	}
	if failed := require(destination, plan.before); failed != nil {
		return journal{}, failed
	}
	if plan.requireExisting && len(plan.before.nodes) == 0 {
		return journal{}, failure(CodeConcurrentChange, "required existing resource disappeared", destination)
	}
	recorded, failed := readReceipt(ctx, st, reference)
	if failed != nil {
		return journal{}, failed
	}
	if !sameOptionalReceipt(recorded, plan.previous) {
		return journal{}, failure(CodeConcurrentChange, "receipt changed during preparation", destination)
	}
	retained, _, failed := transactionRecords(ctx, st, reference)
	if failed != nil {
		return journal{}, failed
	}
	if retained != nil {
		return journal{}, failure(CodeRecoveryNeeded, "transaction record retained", retained.directory)
	}
	prepared, err := newPreparation(reference, plan.receipt.transactionID, directory, stateSchemaVersion)
	if err != nil {
		return journal{}, invalidRecord(err, destination)
	}
	if failed := writeRecord(ctx, st, preparationKey(reference), prepared, nil); failed != nil {
		return journal{}, failed
	}
	ctx = context.WithoutCancel(ctx)
	if failed := inject(ctx, seams.Preparation); failed != nil {
		return journal{}, failed
	}
	if err := sysx.Mkdir(directory, 0o700); err != nil {
		release(ctx, st, prepared)
		if errors.Is(err, syscall.EEXIST) {
			return journal{}, failure(CodeRecoveryNeeded, "unrecognized transaction data retained", directory)
		}
		return journal{}, ioFailure(err, directory)
	}
	recordedJournal, staged, failed := stage(ctx, st, plan, directory)
	if failed != nil {
		discard(ctx, directory, staged)
		if exists, checked := pathExists(directory); checked == nil && !exists {
			release(ctx, st, prepared)
		}
		return journal{}, failed
	}
	if failed := inject(ctx, seams.Prepare); failed != nil {
		return journal{}, failed
	}
	return recordedJournal, nil
}

func stage(ctx context.Context, st store.Store, plan ResourcePlan, directory string) (journal, []revision, *TargetError) {
	var staged []revision
	storageAncestors, failed := storage(plan, directory)
	if failed != nil {
		return journal{}, staged, failed
	}
	if failed := inject(ctx, seams.Directory); failed != nil {
		return journal{}, staged, failed
	}
	if !plan.after.equal(plan.before) {
		stagingPath, _ := transactionPaths(directory)
		written, failed := writeRevision(stagingPath, plan.after)
		if failed != nil {
			return journal{}, staged, failed
		}
		if failed := inject(ctx, seams.Staged); failed != nil {
			return journal{}, staged, failed
		}
		staged = append(staged, written)
	}
	selected := plan.before
	if len(staged) > 0 {
		selected = staged[0]
	}
	recorded, failed := journalFor(ctx, st, plan, directory, storageAncestors, selected)
	if failed != nil {
		return journal{}, staged, failed
	}
	staged = []revision{recorded.staged, recorded.published}
	if failed := touch(directory); failed != nil {
		return journal{}, staged, failed
	}
	if failed := inject(ctx, seams.Journal); failed != nil {
		return journal{}, staged, failed
	}
	if failed := writeRecord(ctx, st, journalKey(plan.resource), recorded, nil); failed != nil {
		return journal{}, staged, failed
	}
	return recorded, staged, nil
}

func ensurePrivate(path string) *TargetError {
	if err := native.EnsurePrivate(path); err != nil {
		return nativeFailure(err)
	}
	return nil
}

func storage(plan ResourcePlan, directory string) ([]ancestor, *TargetError) {
	destination := plan.resource.Destination
	if failed := ensurePrivate(directory); failed != nil {
		return nil, failed
	}
	created := []string{directory, pathx.Join(directory, "staging"), pathx.Join(directory, "backup")}
	expected := make([]revision, 0, 2)
	for _, path := range created[1:] {
		if err := sysx.Mkdir(path, 0o700); err != nil {
			return nil, ioFailure(err, path)
		}
		observed, failed := observe(path)
		if failed != nil {
			return nil, failed
		}
		if failed := ensurePrivate(path); failed != nil {
			return nil, failed
		}
		expected = append(expected, observed)
	}
	storageAncestors, failed := transactionAncestors(destination, directory)
	if failed != nil {
		return nil, failed
	}
	for index, path := range created[1:] {
		if failed := require(path, expected[index]); failed != nil {
			return nil, failed
		}
	}
	retained := slices.DeleteFunc(slices.Clone(storageAncestors), func(item ancestor) bool {
		return slices.ContainsFunc(created, func(path string) bool { return pathsEqual(path, item.path) })
	})
	if !ancestorsEqual(retained, plan.ancestors) {
		return nil, failure(CodeConcurrentChange, "resource ancestor changed", destination)
	}
	return storageAncestors, nil
}

func journalFor(ctx context.Context, st store.Store, plan ResourcePlan, directory string, storageAncestors []ancestor, staged revision) (journal, *TargetError) {
	destination := plan.resource.Destination
	var markers []node
	for _, marker := range plan.receipt.createdDirectories {
		if index := slices.IndexFunc(staged.nodes, func(item node) bool { return item.path == marker.path }); index >= 0 {
			markers = append(markers, staged.nodes[index])
		}
	}
	planned := plan.receipt
	intended, err := newReceipt(planned.resource, planned.transactionID, planned.claims, planned.provenance, markers, stateSchemaVersion)
	if err != nil {
		return journal{}, invalidRecord(err, destination)
	}
	recorded, err := newJournal(journal{
		resource:  plan.resource,
		previous:  plan.previous,
		receipt:   intended,
		before:    plan.before,
		staged:    staged,
		published: staged,
		backup:    plan.before,
		ancestors: storageAncestors,
	}, stateSchemaVersion)
	if err != nil {
		return journal{}, invalidRecord(err, destination)
	}
	if failed := validateAncestors(ctx, st, recorded, directory); failed != nil {
		return journal{}, failed
	}
	if failed := require(destination, plan.before); failed != nil {
		return journal{}, failed
	}
	return recorded, nil
}

// The modification time is the preparation boundary recovery compares against.
func touch(directory string) *TargetError {
	if err := sysx.TouchNow(directory); err != nil {
		return ioFailure(err, directory)
	}
	return nil
}

func publish(ctx context.Context, st store.Store, recorded journal) (committed bool, failed *TargetError) {
	destination := recorded.resource.Destination
	directory, failed := transactionDirectory(ctx, st, recorded)
	if failed != nil {
		return false, failed
	}
	staged, backup := transactionPaths(directory)
	if failed := requireJournal(ctx, st, recorded, directory); failed != nil {
		return false, failed
	}
	if failed := validateAncestors(ctx, st, recorded, directory); failed != nil {
		return false, failed
	}
	if failed := require(destination, recorded.before); failed != nil {
		return false, failed
	}
	if !recorded.before.equal(recorded.staged) {
		if failed := publishMoves(ctx, recorded, staged, backup); failed != nil {
			return false, failed
		}
	}
	if failed := inject(ctx, seams.Publish); failed != nil {
		return false, failed
	}
	if failed := validateAncestors(ctx, st, recorded, directory); failed != nil {
		return false, failed
	}
	if failed := require(destination, recorded.published); failed != nil {
		return false, failed
	}
	return publishReceipt(ctx, st, recorded)
}

func publishMoves(ctx context.Context, recorded journal, staged, backup string) *TargetError {
	destination := recorded.resource.Destination
	if failed := require(staged, recorded.staged); failed != nil {
		return failed
	}
	if len(recorded.before.nodes) > 0 {
		if failed := inject(ctx, seams.BackupMove); failed != nil {
			return failed
		}
		if failed := moveRevision(ctx, destination, backup, recorded.backup); failed != nil {
			return failed
		}
		for _, boundary := range []string{seams.Backup, seams.Rename} {
			if failed := inject(ctx, boundary); failed != nil {
				return failed
			}
		}
		if failed := require(backup, recorded.backup); failed != nil {
			return failed
		}
	}
	if len(recorded.staged.nodes) > 0 {
		for _, boundary := range []string{seams.Staging, seams.Rename} {
			if failed := inject(ctx, boundary); failed != nil {
				return failed
			}
		}
		if failed := moveRevision(ctx, staged, destination, recorded.staged); failed != nil {
			return failed
		}
	}
	return require(destination, recorded.published)
}

func publishReceipt(ctx context.Context, st store.Store, recorded journal) (committed bool, failed *TargetError) {
	destination := recorded.resource.Destination
	if failed := inject(ctx, seams.PublishReceipt); failed != nil {
		return false, failed
	}
	directory, failed := transactionDirectory(ctx, st, recorded)
	if failed != nil {
		return false, failed
	}
	if failed := requireJournal(ctx, st, recorded, directory); failed != nil {
		return false, failed
	}
	if failed := layout(directory); failed != nil {
		return false, failed
	}
	if failed := validateAncestors(ctx, st, recorded, directory); failed != nil {
		return false, failed
	}
	staged, backup := transactionPaths(directory)
	if failed := require(staged, revision{}); failed != nil {
		return false, failed
	}
	if failed := require(destination, recorded.published); failed != nil {
		return false, failed
	}
	expectedBackup := recorded.backup
	if recorded.before.equal(recorded.staged) {
		expectedBackup = revision{}
	}
	if failed := require(backup, expectedBackup); failed != nil {
		return false, failed
	}
	current, failed := currentReceipt(ctx, st, recorded)
	if failed != nil {
		return false, failed
	}
	if !sameOptionalReceipt(current, recorded.previous) {
		return false, failure(CodeConcurrentChange, "receipt changed before commit", destination)
	}
	if failed := inject(ctx, seams.Receipt); failed != nil {
		return false, failed
	}
	if failed := writeRecord(ctx, st, receiptKey(recorded.resource), recorded.receipt, recorded.previous); failed != nil {
		return false, failed
	}
	for _, boundary := range []string{seams.ReceiptReread, seams.ReceiptCommitted} {
		if failed := inject(ctx, boundary); failed != nil {
			return true, failed
		}
	}
	return true, nil
}
