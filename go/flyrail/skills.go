package flyrail

import (
	"context"
	"slices"
	"time"

	"github.com/nielsmadan/flyrail/go/flyrail/store"
)

// SkillOptions are the replacement and lock policy of a skill update or uninstall.
type SkillOptions struct {
	ReplaceModified bool          // false = keep a modified same-owner unit
	LockTimeout     time.Duration // 0 = one acquisition attempt per lock
}

type skillOperation int

const (
	skillInstall skillOperation = iota
	skillUpdate
	skillUninstall
)

// Install installs the skill-only bundle b into each target; a target holding another version of b fails with update_required.
func Install(ctx context.Context, b *Bundle, targets []Target, st store.Store, opts LockOptions) ([]TargetResult, error) {
	if b == nil {
		return nil, newTypeError("install requires a bundle snapshot")
	}
	return skills(ctx, b, b.ID(), targets, st, skillInstall, SkillOptions{LockTimeout: opts.LockTimeout})
}

// Update installs or replaces the skill-only bundle b at each target.
func Update(ctx context.Context, b *Bundle, targets []Target, st store.Store, opts SkillOptions) ([]TargetResult, error) {
	if b == nil {
		return nil, newTypeError("update requires a bundle snapshot")
	}
	return skills(ctx, b, b.ID(), targets, st, skillUpdate, opts)
}

// Uninstall removes everything bundleID installed at each target, without needing the bundle's source.
func Uninstall(ctx context.Context, bundleID string, targets []Target, st store.Store, opts SkillOptions) ([]TargetResult, error) {
	return skills(ctx, nil, bundleID, targets, st, skillUninstall, opts)
}

func skills(ctx context.Context, b *Bundle, identifier string, targets []Target, st store.Store, operation skillOperation, opts SkillOptions) ([]TargetResult, error) {
	if err := checkLockTimeout(opts.LockTimeout); err != nil {
		return nil, err
	}
	prepared, err := prepareSkills(ctx, b, identifier, targets, st)
	if err != nil {
		return nil, err
	}
	results := make([]TargetResult, 0, len(prepared))
	for _, item := range prepared {
		if item.isAlias && item.aliasOf < len(results) {
			results = append(results, aliasResult(results[item.aliasOf], item))
			continue
		}
		result, err := skillResult(ctx, b, identifier, item, operation, opts)
		if err != nil {
			return nil, err
		}
		results = append(results, result)
	}
	return results, nil
}

func aliasResult(original TargetResult, item preparedSkill) TargetResult {
	original.Target, original.IsAlias, original.AliasOf = item.target, true, item.aliasOf
	original.Observation = cloneObservation(original.Observation)
	original.Err = copyTargetError(original.Err)
	original.RecoveryPaths = slices.Clone(original.RecoveryPaths)
	return original
}

func copyTargetError(failed *TargetError) *TargetError {
	if failed == nil {
		return nil
	}
	copied := *failed
	return &copied
}

func skillResult(ctx context.Context, b *Bundle, identifier string, item preparedSkill, operation skillOperation, opts SkillOptions) (TargetResult, error) {
	if item.failed != nil {
		return TargetResult{
			Target:      item.target,
			Root:        item.root,
			Status:      StatusFailed,
			Observation: unknownObservation(item.failed),
			Err:         copyTargetError(item.failed),
		}, nil
	}
	prior, err := InspectInstallation(ctx, identifier, item.installation)
	if err != nil {
		return TargetResult{}, err
	}
	if operation == skillInstall && prior.Version != "" && b != nil &&
		(prior.BundleDigest != b.ContentDigest() || prior.Version != b.Version()) {
		observation, err := skillObservation(ctx, prior, b, item.rendered)
		if err != nil {
			return TargetResult{}, err
		}
		return TargetResult{
			Target:      item.target,
			Root:        item.root,
			Status:      StatusFailed,
			Observation: observation,
			Err:         failure(CodeUpdateRequired, "owned revision requires explicit update", item.root),
		}, nil
	}
	var result InstallationResult
	if b != nil && item.rendered != nil {
		result, err = Sync(ctx, b, item.rendered, item.installation, SyncOptions{Acquisition: AcquireConflict, ReplaceModified: opts.ReplaceModified, LockTimeout: opts.LockTimeout})
	} else {
		result, err = Remove(ctx, identifier, item.installation, RemoveOptions(opts))
	}
	if err != nil {
		return TargetResult{}, err
	}
	observation, err := skillObservation(ctx, result.Observation, b, item.rendered)
	if err != nil {
		return TargetResult{}, err
	}
	var recoveryPaths []string
	for _, resource := range result.Resources {
		recoveryPaths = append(recoveryPaths, resource.RecoveryPaths...)
	}
	return TargetResult{
		Target:        item.target,
		Root:          item.root,
		Status:        result.Status,
		Observation:   observation,
		Err:           result.Err,
		RecoveryPaths: recoveryPaths,
	}, nil
}
