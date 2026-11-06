package flyrail

import (
	"context"
	"slices"
	"syscall"
	"time"

	"github.com/nielsmadan/flyrail/go/flyrail/store"
	"github.com/nielsmadan/flyrail/go/internal/native"
	"github.com/nielsmadan/flyrail/go/internal/pathx"
	"github.com/nielsmadan/flyrail/go/internal/seams"
	"github.com/nielsmadan/flyrail/go/internal/sysx"
)

func readReceipt(ctx context.Context, st store.Store, reference ResourceRef) (*receipt, *TargetError) {
	recorded, ok, failed := readRecord(ctx, st, receiptKey(reference), "Receipt", receiptFromState)
	if failed != nil || !ok {
		return nil, failed
	}
	return &recorded, nil
}

func currentReceipt(ctx context.Context, st store.Store, recorded journal) (*receipt, *TargetError) {
	return readReceipt(ctx, st, recorded.resource)
}

func preparationFor(ctx context.Context, st store.Store, recorded journal) (preparation, *TargetError) {
	prepared, ok, failed := readRecord(ctx, st, preparationKey(recorded.resource), "Preparation", preparationFromState)
	if failed != nil {
		return preparation{}, failed
	}
	var selected *preparation
	if ok {
		if !knownPreparation(st, prepared) {
			return preparation{}, unrecognizedDirectory(st)
		}
		selected = &prepared
	}
	return matchingPreparation(st, selected, recorded)
}

func transactionDirectory(ctx context.Context, st store.Store, recorded journal) (string, *TargetError) {
	prepared, failed := preparationFor(ctx, st, recorded)
	if failed != nil {
		return "", failed
	}
	return prepared.directory, nil
}

func pathExists(path string) (bool, *TargetError) {
	_, err := sysx.LstatMetadata(path)
	switch {
	case err == nil:
		return true, nil
	case notFound(err):
		return false, nil
	}
	return false, ioFailure(err, path)
}

func children(directory string) ([]string, *TargetError) {
	names, err := sysx.ReadDirNames(directory)
	switch {
	case notFound(err):
		return nil, nil
	case err != nil:
		return nil, ioFailure(err, directory)
	}
	result := make([]string, 0, len(names))
	for _, name := range names {
		result = append(result, pathx.Join(directory, name))
	}
	slices.SortFunc(result, comparePaths)
	return result, nil
}

func layout(directory string) *TargetError {
	if err := native.ManagedSpelling(directory); err != nil {
		return nativeFailure(err)
	}
	entries, failed := children(directory)
	if failed != nil {
		return failed
	}
	for _, child := range entries {
		if name, _ := pathx.FileName(child); name != "staging" && name != "backup" {
			return failure(CodeRecoveryNeeded, "unknown transaction data", child)
		}
		metadata, err := sysx.LstatMetadata(child)
		if err != nil {
			return ioFailure(err, child)
		}
		if failed := managedMetadata(metadata, child); failed != nil {
			return failed
		}
		if !metadata.IsDir() {
			return failure(CodeUnsafePath, "expected a directory", child)
		}
		contents, failed := children(child)
		if failed != nil {
			return failed
		}
		if slices.ContainsFunc(contents, func(entry string) bool {
			name, ok := pathx.FileName(entry)
			return !ok || name != "resource"
		}) {
			return failure(CodeRecoveryNeeded, "unknown transaction data", child)
		}
	}
	return nil
}

func transactionAncestors(destination, directory string) ([]ancestor, *TargetError) {
	staged, backup := transactionPaths(directory)
	return resourceAncestors(destination, staged, backup)
}

func pathWithin(path, base string) bool {
	components, prefix := pathx.Components(path), pathx.Components(base)
	return len(components) >= len(prefix) && slices.Equal(components[:len(prefix)], prefix)
}

func ensureExistingPrivate(directory string) *TargetError {
	for _, path := range []string{directory, pathx.Join(directory, "staging"), pathx.Join(directory, "backup")} {
		exists, failed := pathExists(path)
		if failed != nil {
			return failed
		}
		if exists {
			if failed := ensurePrivate(path); failed != nil {
				return failed
			}
		}
	}
	return nil
}

func validateAncestors(ctx context.Context, st store.Store, recorded journal, directory string) *TargetError {
	reference := recorded.resource
	current, failed := transactionAncestors(reference.Destination, directory)
	if failed != nil {
		return failed
	}
	retained := slices.DeleteFunc(slices.Clone(recorded.ancestors), func(item ancestor) bool {
		return !slices.ContainsFunc(current, item.equal) && pathWithin(item.path, directory)
	})
	if !ancestorsEqual(current, retained) {
		return failure(CodeConcurrentChange, "resource ancestor changed", reference.Destination)
	}
	if failed := ensureExistingPrivate(directory); failed != nil {
		return failed
	}
	authority, failed := authorityChecked(ctx, reference)
	if failed != nil {
		return failed
	}
	conflicts, failed := hierarchyConflicts(ctx, st, authority, reference.Kind != ResourceDocument)
	if failed != nil {
		return failed
	}
	if len(conflicts) > 0 {
		return failure(CodeConflict, "resource hierarchy reservation changed", conflicts[0])
	}
	return nil
}

func requireJournal(ctx context.Context, st store.Store, recorded journal, directory string) *TargetError {
	current, ok, failed := readRecord(ctx, st, journalKey(recorded.resource), "Journal", journalFromState)
	if failed != nil {
		return failed
	}
	if !ok || !sameRecord(current, recorded) {
		return failure(CodeRecoveryNeeded, "transaction record changed", directory)
	}
	return nil
}

func removeDirectories(directory string, ignoreFailures bool) *TargetError {
	for _, path := range []string{pathx.Join(directory, "staging"), pathx.Join(directory, "backup"), directory} {
		if err := sysx.Rmdir(path); err != nil && !notFound(err) && !ignoreFailures {
			return ioFailure(err, path)
		}
	}
	return nil
}

func release(ctx context.Context, st store.Store, prepared preparation) {
	if _, present, err := st.Read(ctx, journalKey(prepared.resource)); err == nil && !present {
		_ = deleteRecord(ctx, st, preparationKey(prepared.resource), prepared)
	}
}

func discard(ctx context.Context, directory string, revisions []revision) {
	staged, _ := transactionPaths(directory)
	current, failed := observe(staged)
	if failed == nil && len(current.nodes) > 0 && slices.ContainsFunc(revisions, current.equal) {
		failed = destroy(ctx, staged, current)
	}
	if failed == nil {
		_ = removeDirectories(directory, true)
	}
}

func cleanup(ctx context.Context, st store.Store, recorded journal) *TargetError {
	prepared, failed := preparationFor(ctx, st, recorded)
	if failed != nil {
		return failed
	}
	directory := prepared.directory
	if failed := requireJournal(ctx, st, recorded, directory); failed != nil {
		return failed
	}
	if failed := layout(directory); failed != nil {
		return failed
	}
	if failed := validateAncestors(ctx, st, recorded, directory); failed != nil {
		return failed
	}
	staged, backup := transactionPaths(directory)
	areas := []struct {
		path     string
		expected []revision
	}{
		{staged, []revision{recorded.staged, recorded.published}},
		{backup, []revision{recorded.backup, recorded.before}},
	}
	for _, area := range areas {
		current, failed := observe(area.path)
		if failed != nil {
			return failed
		}
		if len(current.nodes) == 0 {
			continue
		}
		if !slices.ContainsFunc(area.expected, func(expected revision) bool { return cleanupRevision(current, expected) }) {
			return failure(CodeRecoveryNeeded, "unexpected cleanup revision", area.path)
		}
		if failed := destroy(ctx, area.path, current); failed != nil {
			return failed
		}
	}
	if failed := removeDirectories(directory, false); failed != nil {
		return failed
	}
	if failed := inject(ctx, seams.Cleanup); failed != nil {
		return failed
	}
	if failed := deleteRecord(ctx, st, journalKey(recorded.resource), recorded); failed != nil {
		return failed
	}
	return deleteRecord(ctx, st, preparationKey(recorded.resource), prepared)
}

func preparationTime(directory string) (time.Time, *TargetError) {
	metadata, err := sysx.LstatMetadata(directory)
	if err != nil {
		return time.Time{}, ioFailure(err, directory)
	}
	return time.Unix(metadata.MtimeSec, metadata.MtimeNsec), nil
}

func rollback(ctx context.Context, st store.Store, recorded journal) *TargetError {
	destination := recorded.resource.Destination
	directory, failed := transactionDirectory(ctx, st, recorded)
	if failed != nil {
		return failed
	}
	stagedPath, backupPath := transactionPaths(directory)
	if failed := layout(directory); failed != nil {
		return failed
	}
	if failed := validateAncestors(ctx, st, recorded, directory); failed != nil {
		return failed
	}
	actual, failed := observe(destination)
	if failed != nil {
		return failed
	}
	staged, failed := observe(stagedPath)
	if failed != nil {
		return failed
	}
	backup, failed := observe(backupPath)
	if failed != nil {
		return failed
	}
	if failed := requireUntouched(recorded, directory, actual); failed != nil {
		return failed
	}
	if actual.equal(recorded.before) && len(backup.nodes) == 0 {
		if failed := inject(ctx, seams.Rollback); failed != nil {
			return failed
		}
		return cleanup(ctx, st, recorded)
	}
	if len(staged.nodes) > 0 && !staged.equal(recorded.staged) && !staged.equal(recorded.published) {
		return failure(CodeRecoveryNeeded, "unexpected staged revision", stagedPath)
	}
	if len(backup.nodes) > 0 && !backup.equal(recorded.before) && !backup.equal(recorded.backup) {
		return failure(CodeRecoveryNeeded, "unexpected backup revision", backupPath)
	}
	if failed := restore(ctx, recorded, actual, staged, backup, stagedPath, backupPath); failed != nil {
		return failed
	}
	if failed := inject(ctx, seams.Rollback); failed != nil {
		return failed
	}
	return cleanup(ctx, st, recorded)
}

func requireUntouched(recorded journal, directory string, actual revision) *TargetError {
	if len(actual.nodes) == 0 || !actual.equal(recorded.before) && !actual.equal(recorded.staged) && !actual.equal(recorded.published) {
		return nil
	}
	exists, failed := pathExists(directory)
	if failed != nil || !exists {
		return failed
	}
	boundary, failed := preparationTime(directory)
	if failed != nil {
		return failed
	}
	return requireUnchangedSince(recorded.resource.Destination, actual, boundary)
}

func restore(ctx context.Context, recorded journal, actual, staged, backup revision, stagedPath, backupPath string) *TargetError {
	destination := recorded.resource.Destination
	switch {
	case recorded.before.equal(recorded.staged):
		return require(destination, recorded.before)
	case len(backup.nodes) > 0:
		if len(actual.nodes) > 0 {
			if !actual.equal(recorded.published) && !actual.equal(recorded.staged) || len(staged.nodes) > 0 {
				return failure(CodeRecoveryNeeded, "unrecognized published revision", destination)
			}
			if failed := moveRevision(ctx, destination, stagedPath, actual); failed != nil {
				return failed
			}
		}
		if failed := moveRevision(ctx, backupPath, destination, backup); failed != nil {
			return failed
		}
		return require(destination, recorded.before)
	case actual.equal(recorded.before):
		return nil
	case len(recorded.before.nodes) == 0 && actual.equal(recorded.published) && len(staged.nodes) == 0:
		return moveRevision(ctx, destination, stagedPath, actual)
	}
	return failure(CodeRecoveryNeeded, "cannot restore complete previous revision", destination)
}

func abandonedRevisions(directory string) (retained, current revision, failed *TargetError) {
	if failed := layout(directory); failed != nil {
		return revision{}, revision{}, failed
	}
	if failed := ensureExistingPrivate(directory); failed != nil {
		return revision{}, revision{}, failed
	}
	staged, backup := transactionPaths(directory)
	if retained, failed = observe(backup); failed != nil {
		return revision{}, revision{}, failed
	}
	if current, failed = observe(staged); failed != nil {
		return revision{}, revision{}, failed
	}
	return retained, current, nil
}

func abandon(ctx context.Context, st store.Store, prepared preparation) *TargetError {
	directory := prepared.directory
	exists, failed := pathExists(directory)
	if failed != nil {
		return failed
	}
	if exists {
		if !knownPreparation(st, prepared) {
			return unrecognizedDirectory(st)
		}
		retained, current, failed := abandonedRevisions(directory)
		if failed != nil {
			if failed.Errno == 0 || failed.Errno == syscall.ENOTSUP {
				return failure(CodeRecoveryNeeded, "unrecognized transaction data retained", directory)
			}
			return failed
		}
		if len(retained.nodes) > 0 {
			return failure(CodeRecoveryNeeded, "unrecognized transaction data retained", directory)
		}
		staged, _ := transactionPaths(directory)
		if failed := destroy(ctx, staged, current); failed != nil {
			return failed
		}
		if failed := removeDirectories(directory, false); failed != nil {
			return failed
		}
	}
	return deleteRecord(ctx, st, preparationKey(prepared.resource), prepared)
}

// Ignores cancellation: an interrupted transaction must be resolved once started.
func recoverResource(ctx context.Context, st store.Store, guard *resourceLock) *TargetError {
	ctx = context.WithoutCancel(ctx)
	reference := guard.reference
	prepared, recorded, failed := transactionRecords(ctx, st, reference)
	if failed != nil || prepared == nil {
		return failed
	}
	if !resourceRefsEqual(prepared.resource, reference) {
		return failure(CodeRecoveryNeeded, "transaction record disagrees", prepared.directory)
	}
	if recorded == nil {
		return abandon(ctx, st, *prepared)
	}
	if recorded.previous != nil && sameRecord(*recorded.previous, recorded.receipt) {
		return failure(CodeRecoveryNeeded, "transaction record repeats the previous receipt", prepared.directory)
	}
	current, failed := currentReceipt(ctx, st, *recorded)
	switch {
	case failed != nil:
		return failed
	case current != nil && sameRecord(*current, recorded.receipt):
		return cleanup(ctx, st, *recorded)
	case sameOptionalReceipt(current, recorded.previous):
		return rollback(ctx, st, *recorded)
	}
	return failure(CodeRecoveryNeeded, "receipt disagrees with transaction", reference.Destination)
}
