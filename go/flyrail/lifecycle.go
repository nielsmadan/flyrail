package flyrail

import (
	"context"
	"time"
)

func requireContext(ctx context.Context) error {
	if ctx == nil {
		return newValueError("context must not be nil")
	}
	return nil
}

func checkLockTimeout(timeout time.Duration) error {
	if timeout < 0 {
		return newValueError("lock timeout must be finite and nonnegative")
	}
	return nil
}

func acquisitionOf(acquisition Acquisition) (Acquisition, error) {
	if acquisition == "" {
		return AcquireConflict, nil
	}
	if _, err := ParseAcquisition(string(acquisition)); err != nil {
		return "", newTypeError("invalid acquisition or replacement policy")
	}
	return acquisition, nil
}

func installationRequest(operation string, b *Bundle, r *RenderedBundle, t *InstallationTarget, acquisition Acquisition, replaceModified bool) (previewRequest, error) {
	if b == nil || r == nil || !t.valid() {
		return previewRequest{}, newTypeError(operation + " requires a bundle snapshot, rendered bundle and installation target")
	}
	selected, err := acquisitionOf(acquisition)
	if err != nil {
		return previewRequest{}, err
	}
	return previewRequest{
		identifier:      b.ID(),
		bundle:          b,
		rendered:        r,
		target:          t,
		acquisition:     selected,
		replaceModified: replaceModified,
	}, nil
}

func removalRequest(bundleID string, t *InstallationTarget, replaceModified bool) (previewRequest, error) {
	if err := validateBundleID(bundleID); err != nil {
		return previewRequest{}, err
	}
	if !t.valid() {
		return previewRequest{}, newTypeError("invalid installation target or replacement policy")
	}
	return previewRequest{
		identifier:      bundleID,
		rendered:        EmptyRenderedBundle(),
		target:          t,
		acquisition:     AcquireConflict,
		replaceModified: replaceModified,
	}, nil
}

func observedTarget(bundleID string, t *InstallationTarget) error {
	if err := validateBundleID(bundleID); err != nil {
		return err
	}
	if !t.valid() {
		return newTypeError("target must be an InstallationTarget")
	}
	return nil
}

// Preview observes t and plans installing b rendered as r, without changing anything.
func Preview(ctx context.Context, b *Bundle, r *RenderedBundle, t *InstallationTarget, opts PreviewOptions) (*LifecyclePreview, error) {
	if err := requireContext(ctx); err != nil {
		return nil, err
	}
	request, err := installationRequest("preview", b, r, t, opts.Acquisition, opts.ReplaceModified)
	if err != nil {
		return nil, err
	}
	return publicPreview(ctx, request)
}

// PreviewRemoval observes t and plans removing everything bundleID owns there, without changing anything.
func PreviewRemoval(ctx context.Context, bundleID string, t *InstallationTarget, replaceModified bool) (*LifecyclePreview, error) {
	if err := requireContext(ctx); err != nil {
		return nil, err
	}
	request, err := removalRequest(bundleID, t, replaceModified)
	if err != nil {
		return nil, err
	}
	return publicPreview(ctx, request)
}

// ApplyPreview applies p exactly as planned, failing without changes if anything it observed has changed.
func ApplyPreview(ctx context.Context, p *LifecyclePreview, opts LockOptions) InstallationResult {
	if p == nil || !p.target.valid() || p.rendered == nil || ctx == nil {
		message := "apply requires a LifecyclePreview"
		if ctx == nil {
			message = "context must not be nil"
		}
		return InstallationResult{
			Status:      StatusFailed,
			Observation: InstallationObservation{BundleID: p.BundleID(), Target: p.Target()},
			Err:         newTargetError(CodeInvalidState, message, "", 0, nil),
		}
	}
	if err := checkLockTimeout(opts.LockTimeout); err != nil {
		return failedResult(ctx, p.bundleID, p.target, newTargetError(CodeInvalidState, err.Error(), "", 0, err), nil)
	}
	return apply(ctx, p, opts.LockTimeout)
}

// Sync recovers the installation of b at t and converges it on b rendered as r.
func Sync(ctx context.Context, b *Bundle, r *RenderedBundle, t *InstallationTarget, opts SyncOptions) (InstallationResult, error) {
	if err := requireContext(ctx); err != nil {
		return InstallationResult{}, err
	}
	request, err := installationRequest("sync", b, r, t, opts.Acquisition, opts.ReplaceModified)
	if err != nil {
		return InstallationResult{}, err
	}
	if err := checkLockTimeout(opts.LockTimeout); err != nil {
		return InstallationResult{}, err
	}
	failed, err := validateReferenceRequest(ctx, r, t, b)
	if err != nil {
		return InstallationResult{}, err
	}
	if failed != nil {
		return failedResult(ctx, b.ID(), t, failed, r.Notices()), nil
	}
	return synchronize(ctx, request, opts.LockTimeout), nil
}

// RecoverInstallation completes or rolls back the interrupted transactions of bundleID at t.
func RecoverInstallation(ctx context.Context, bundleID string, t *InstallationTarget, opts LockOptions) (InstallationResult, error) {
	if err := requireContext(ctx); err != nil {
		return InstallationResult{}, err
	}
	if err := observedTarget(bundleID, t); err != nil {
		return InstallationResult{}, err
	}
	if err := checkLockTimeout(opts.LockTimeout); err != nil {
		return InstallationResult{}, err
	}
	return recoverInstallation(ctx, bundleID, t, opts.LockTimeout), nil
}

// InspectInstallation observes the recorded installation of bundleID at t without taking locks.
func InspectInstallation(ctx context.Context, bundleID string, t *InstallationTarget) (InstallationObservation, error) {
	if err := requireContext(ctx); err != nil {
		return InstallationObservation{}, err
	}
	if err := observedTarget(bundleID, t); err != nil {
		return InstallationObservation{}, err
	}
	return inspect(ctx, bundleID, t), nil
}

// Remove recovers the installation of bundleID at t and removes everything it owns there.
func Remove(ctx context.Context, bundleID string, t *InstallationTarget, opts RemoveOptions) (InstallationResult, error) {
	if err := requireContext(ctx); err != nil {
		return InstallationResult{}, err
	}
	request, err := removalRequest(bundleID, t, opts.ReplaceModified)
	if err != nil {
		return InstallationResult{}, err
	}
	if err := checkLockTimeout(opts.LockTimeout); err != nil {
		return InstallationResult{}, err
	}
	return synchronize(ctx, request, opts.LockTimeout), nil
}
