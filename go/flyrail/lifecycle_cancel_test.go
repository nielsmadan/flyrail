//go:build darwin || linux

package flyrail

import (
	"context"
	"errors"
	"path/filepath"
	"testing"

	"github.com/nielsmadan/flyrail/go/flyrail/store"
	"github.com/nielsmadan/flyrail/go/internal/seams"
)

func cancelAt(cancel context.CancelFunc, boundary string) *seams.Hooks {
	return &seams.Hooks{Observe: func(event seams.Event) {
		if event.Kind == seams.KindBoundary && event.Boundary == boundary {
			cancel()
		}
	}}
}

func interruptedInstall(t *testing.T) (target *InstallationTarget, destination string) {
	t.Helper()
	destination = filepath.Join(tempRoot(t), "AGENTS.md")
	target = targetOf(t, &strictStore{store.NewMemoryStore()})
	interruptedSync := syncWith(seams.With(t.Context(), faultAt(seams.Cleanup)), t, teamBundle(t, "1"), sectionRendered(t, destination, versionText("1")), target, SyncOptions{LockTimeout: testLockTimeout})
	wantStatus(t, "interrupted", interruptedSync, StatusPartial)
	if !inspectTeam(t, target).Pending {
		t.Fatal("the interrupted installation is not pending")
	}
	return target, destination
}

func TestASyncCancelledAfterTheReceiptCommitsStillReportsTheConvergedInstallation(t *testing.T) {
	t.Parallel()
	destination := filepath.Join(tempRoot(t), "AGENTS.md")
	target := targetOf(t, &strictStore{store.NewMemoryStore()})
	bundle, rendered := teamBundle(t, "1"), sectionRendered(t, destination, versionText("1"))
	ctx, cancel := context.WithCancel(t.Context())
	result := syncWith(seams.With(ctx, cancelAt(cancel, seams.ReceiptCommitted)), t, bundle, rendered, target, SyncOptions{LockTimeout: testLockTimeout})
	wantStatus(t, "sync", result, StatusApplied)
	if !result.Observation.Matches(bundle, rendered) {
		t.Errorf("observation = %+v, want the settled installation", result.Observation)
	}
	if !inspectTeam(t, target).Matches(bundle, rendered) {
		t.Error("the stored installation is not settled")
	}
}

func TestAFailedSyncObservesWithoutTheCancelledContext(t *testing.T) {
	t.Parallel()
	destination := filepath.Join(tempRoot(t), "AGENTS.md")
	target := targetOf(t, &strictStore{store.NewMemoryStore()})
	ctx, cancel := context.WithCancel(t.Context())
	result := syncWith(seams.With(ctx, cancelAt(cancel, seams.ResourcePrecondition)), t, teamBundle(t, "1"), sectionRendered(t, destination, versionText("1")), target, SyncOptions{LockTimeout: testLockTimeout})
	wantStatus(t, "sync", result, StatusFailed)
	if result.Observation.Err != nil {
		t.Errorf("observation error = %v, want an observation made without the cancelled context", result.Observation.Err)
	}
}

func TestRecoveryCancelledMidCleanupStillSettlesEveryRecord(t *testing.T) {
	t.Parallel()
	target, destination := interruptedInstall(t)
	ctx, cancel := context.WithCancel(t.Context())
	recovered, err := RecoverInstallation(seams.With(ctx, cancelAt(cancel, seams.Cleanup)), "team", target, LockOptions{LockTimeout: testLockTimeout})
	if err != nil {
		t.Fatal(err)
	}
	wantStatus(t, "recover", recovered, StatusApplied)
	assertSettled(t, target.Store(), documentRef(t, destination))
}

func TestASyncCancelledWhileRecoveringSettlesTheRecoveryThenRefusesToStartTheRequest(t *testing.T) {
	t.Parallel()
	target, destination := interruptedInstall(t)
	ctx, cancel := context.WithCancel(t.Context())
	result := syncWith(seams.With(ctx, cancelAt(cancel, seams.Cleanup)), t, teamBundle(t, "2"), sectionRendered(t, destination, versionText("2")), target, SyncOptions{LockTimeout: testLockTimeout})
	wantStatus(t, "sync", result, StatusFailed)
	if !errors.Is(result.Failure(), context.Canceled) {
		t.Errorf("failure = %v, want a cancellation", result.Failure())
	}
	assertSettled(t, target.Store(), documentRef(t, destination))
}

func TestASyncWhoseContextEndedBeforeTheCallObservesWithoutTheCancelledContext(t *testing.T) {
	t.Parallel()
	destination := filepath.Join(tempRoot(t), "AGENTS.md")
	target := targetOf(t, &strictStore{store.NewMemoryStore()})
	ctx, cancel := context.WithCancel(t.Context())
	cancel()
	result := syncWith(ctx, t, teamBundle(t, "1"), sectionRendered(t, destination, versionText("1")), target, SyncOptions{LockTimeout: testLockTimeout})
	wantStatus(t, "sync", result, StatusFailed)
	if !errors.Is(result.Failure(), context.Canceled) {
		t.Errorf("failure = %v, want a cancellation", result.Failure())
	}
	if result.Observation.Err != nil {
		t.Errorf("observation error = %v, want an observation made without the cancelled context", result.Observation.Err)
	}
}

func TestRecoveryCancelledMidCleanupReportsTheSettledObservation(t *testing.T) {
	t.Parallel()
	target, _ := interruptedInstall(t)
	ctx, cancel := context.WithCancel(t.Context())
	recovered, err := RecoverInstallation(seams.With(ctx, cancelAt(cancel, seams.Cleanup)), "team", target, LockOptions{LockTimeout: testLockTimeout})
	if err != nil {
		t.Fatal(err)
	}
	wantStatus(t, "recover", recovered, StatusApplied)
	if recovered.Observation.Err != nil {
		t.Errorf("observation error = %v, want an observation made without the cancelled context", recovered.Observation.Err)
	}
	if want := inspectTeam(t, target); recovered.Observation.Pending != want.Pending {
		t.Errorf("observation pending = %t, want %t as inspected afterwards", recovered.Observation.Pending, want.Pending)
	}
}
