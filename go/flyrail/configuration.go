package flyrail

import (
	"slices"
	"time"
)

// PreviewOptions are the acquisition and replacement policy of a preview.
type PreviewOptions struct {
	Acquisition     Acquisition // "" = AcquireConflict
	ReplaceModified bool        // false = keep modified owned content
}

// SyncOptions are the acquisition, replacement and lock policy of a sync.
type SyncOptions struct {
	Acquisition     Acquisition   // "" = AcquireConflict
	ReplaceModified bool          // false = keep modified owned content
	LockTimeout     time.Duration // 0 = one acquisition attempt per lock
}

// RemoveOptions are the replacement and lock policy of a removal.
type RemoveOptions struct {
	ReplaceModified bool          // false = keep modified owned content
	LockTimeout     time.Duration // 0 = one acquisition attempt per lock
}

// LockOptions are the lock policy of an operation that takes locks.
type LockOptions struct {
	LockTimeout time.Duration // 0 = one acquisition attempt per lock
}

// ResourceObservation is the observed state of one resource of an installation.
type ResourceObservation struct {
	Destination    string
	Claims         []ClaimObservation
	RevisionDigest string
	Err            *TargetError
	RecoveryPaths  []string
}

// Failure returns Err as an error, or a nil error when there is no failure.
func (o ResourceObservation) Failure() error {
	return failureOf(o.Err)
}

// InstallationObservation is the observed state of one bundle's installation at a target.
type InstallationObservation struct {
	BundleID     string
	Target       *InstallationTarget
	Resources    []ResourceObservation
	Current      bool
	Pending      bool
	Version      string
	BundleDigest string
	RenderDigest string
	Err          *TargetError
}

// Failure returns Err as an error, or a nil error when there is no failure.
func (o InstallationObservation) Failure() error {
	return failureOf(o.Err)
}

// Matches reports whether the observation is a current, settled installation of exactly b rendered as r.
func (o InstallationObservation) Matches(b *Bundle, r *RenderedBundle) bool {
	return b != nil && r != nil &&
		o.Current && !o.Pending && o.Err == nil && r.Supported() &&
		o.BundleID == b.ID() &&
		o.Version != "" && o.Version == b.Version() &&
		o.BundleDigest != "" && o.BundleDigest == b.ContentDigest() &&
		o.RenderDigest != "" && o.RenderDigest == r.ContentDigest()
}

// ResourceResult is the outcome of an operation for one resource.
type ResourceResult struct {
	Resource      ResourceRef
	Status        OperationStatus
	Err           *TargetError
	RecoveryPaths []string
}

// Succeeded reports whether the resource's status is applied or unchanged.
func (r ResourceResult) Succeeded() bool {
	return succeeded(r.Status)
}

// Failure returns Err as an error, or a nil error when there is no failure.
func (r ResourceResult) Failure() error {
	return failureOf(r.Err)
}

// InstallationResult is the outcome of a lifecycle operation for one installation.
type InstallationResult struct {
	Status      OperationStatus
	Observation InstallationObservation
	Resources   []ResourceResult
	Notices     []Notice
	Err         *TargetError
}

// Succeeded reports whether the operation's status is applied or unchanged.
func (r InstallationResult) Succeeded() bool {
	return succeeded(r.Status)
}

// Failure returns Err as an error, or a nil error when there is no failure.
func (r InstallationResult) Failure() error {
	return failureOf(r.Err)
}

// LifecyclePreview is an immutable plan bound to every precondition it observed.
type LifecyclePreview struct {
	bundleID          string
	bundle            *Bundle
	rendered          *RenderedBundle
	target            *InstallationTarget
	acquisition       Acquisition
	replaceModified   bool
	index             *index
	generation        *generation
	resources         []ResourcePlan
	err               *TargetError
	readPreconditions []loadoutInput
}

// BundleID returns the bundle id the preview installs or removes.
func (p *LifecyclePreview) BundleID() string {
	if p == nil {
		return ""
	}
	return p.bundleID
}

// Bundle returns the bundle the preview installs, or nil for a removal.
func (p *LifecyclePreview) Bundle() *Bundle {
	if p == nil {
		return nil
	}
	return p.bundle
}

// Rendered returns the rendered bundle the preview installs.
func (p *LifecyclePreview) Rendered() *RenderedBundle {
	if p == nil {
		return nil
	}
	return p.rendered
}

// Target returns the installation target.
func (p *LifecyclePreview) Target() *InstallationTarget {
	if p == nil {
		return nil
	}
	return p.target
}

// Acquisition returns the acquisition policy.
func (p *LifecyclePreview) Acquisition() Acquisition {
	if p == nil {
		return ""
	}
	return p.acquisition
}

// ReplaceModified reports whether modified owned content is replaced.
func (p *LifecyclePreview) ReplaceModified() bool {
	return p != nil && p.replaceModified
}

// Resources returns the resource plans in application order.
func (p *LifecyclePreview) Resources() []ResourcePlan {
	if p == nil {
		return nil
	}
	return slices.Clone(p.resources)
}

// Err returns a copy of the failure that prevents applying the preview, or nil.
func (p *LifecyclePreview) Err() *TargetError {
	if p == nil {
		return nil
	}
	return copyTargetError(p.err)
}

// Failure returns Err() as an error, or a nil error when there is no failure.
func (p *LifecyclePreview) Failure() error {
	return failureOf(p.Err())
}

// Applicable reports whether the preview has no failure and no resource needing recovery.
func (p *LifecyclePreview) Applicable() bool {
	return p != nil && p.err == nil && !slices.ContainsFunc(p.resources, func(plan ResourcePlan) bool {
		return plan.err != nil || len(plan.recoveryPaths) > 0
	})
}

func (p *LifecyclePreview) withResources(resources []ResourcePlan) *LifecyclePreview {
	copied := *p
	copied.resources = resources
	return &copied
}

func validateBundleID(bundleID string) error {
	return validateIdentifier(bundleID, "bundle id")
}
