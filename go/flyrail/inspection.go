package flyrail

import (
	"context"
	"errors"
	"slices"
	"strings"

	"github.com/nielsmadan/flyrail/go/flyrail/store"
)

type preparedSkill struct {
	target       Target
	installation *InstallationTarget
	failed       *TargetError
	root         string
	isAlias      bool
	aliasOf      int
	rendered     *RenderedBundle
}

// RenderSkills renders a skill-only bundle into the skill container of t, or into unsupported notices for an agent without a verified translation.
func RenderSkills(ctx context.Context, b *Bundle, t Target) (*RenderedBundle, error) {
	if err := requireContext(ctx); err != nil {
		return nil, err
	}
	if b == nil {
		return nil, invalidAuthority(newTypeError("render requires a bundle snapshot"))
	}
	if err := validateSkillBundle(b); err != nil {
		return nil, invalidAuthority(err)
	}
	if err := checkSkillTarget(t); err != nil {
		return nil, invalidAuthority(err)
	}
	if t.agent != "" && !IsTranslatable(t.agent) {
		notices := make([]Notice, 0, len(b.skills))
		for _, skill := range b.skills {
			notices = append(notices, Notice{ArtifactID: skill.name, Kind: NoticeUnsupported, Message: UntranslatableMessage(t.agent), Code: "agent-unsupported"})
		}
		rendered, err := newRenderedBundle(nil, nil, notices, nil)
		if err != nil {
			return nil, asAuthorityError(err)
		}
		return rendered, nil
	}
	authority, err := NewResourceAuthority(ctx, t.root)
	if err != nil {
		return nil, err
	}
	rendered, err := renderSkillBundleAt(b, authority.destination)
	if err != nil {
		return nil, asAuthorityError(err)
	}
	return rendered, nil
}

func asAuthorityError(err error) *AuthorityError {
	if failure, ok := errors.AsType[*ValueError](err); ok {
		return invalidAuthority(failure)
	}
	return &AuthorityError{Message: err.Error(), Err: err}
}

func checkSkillTarget(t Target) *ValueError {
	if t.root == "" {
		return newTypeError("targets must be Target values")
	}
	return nil
}

func validateSkillBundle(b *Bundle) *ValueError {
	if slices.ContainsFunc(b.artifacts, func(artifact Artifact) bool {
		_, skill := artifact.(SkillArtifact)
		return !skill
	}) {
		return newValueError("use explicit rendering for non-skill artifacts")
	}
	if len(b.assets) > 0 || len(b.dependencies) > 0 {
		return newValueError("use explicit rendering for skill bundles with assets or dependencies")
	}
	return nil
}

func renderSkillBundleAt(b *Bundle, destination string) (*RenderedBundle, error) {
	var rendered []RenderedArtifact
	for _, skill := range b.skills {
		prefix := skill.name + "/"
		var entries []BundleEntry
		for _, entry := range b.entries {
			if path, ok := strings.CutPrefix(entry.Path, prefix); ok {
				entries = append(entries, BundleEntry{Path: path, Data: entry.Data, Dir: entry.Dir, Executable: entry.Executable})
			}
		}
		content, err := NewTreeContent(entries)
		if err != nil {
			return nil, err
		}
		artifact, err := NewRenderedArtifact(RenderedArtifactParams{ID: skill.name, Family: FamilySkills, Destination: destination, Content: content, Subtree: skill.name})
		if err != nil {
			return nil, err
		}
		rendered = append(rendered, artifact)
	}
	if len(rendered) == 0 {
		for _, artifact := range b.artifacts {
			skill, ok := artifact.(SkillArtifact)
			if !ok {
				continue
			}
			item, err := NewRenderedArtifact(RenderedArtifactParams{ID: skill.id, Family: FamilySkills, Destination: destination, Content: skill.content, Subtree: skill.name})
			if err != nil {
				return nil, err
			}
			rendered = append(rendered, item)
		}
	}
	return newRenderedBundle(rendered, nil, nil, nil)
}

func skillAuthorityFailure(err error) (*TargetError, error) {
	if authority, ok := errors.AsType[*AuthorityError](err); ok {
		if failure, invalid := errors.AsType[*ValueError](authority.Err); invalid {
			return nil, failure
		}
	}
	return errorDetail(err), nil
}

func skillRouting(t Target, root string) map[string]string {
	routing := map[string]string{
		"destination": root,
		"agent":       string(t.agent),
		"scope":       string(t.scope),
		"home":        t.home,
	}
	for _, entry := range t.environment {
		routing["env:"+entry.key] = entry.value
	}
	return routing
}

func prepareTarget(ctx context.Context, t Target, st store.Store) (preparedSkill, error) {
	if t.agent != "" && !IsTranslatable(t.agent) {
		return preparedSkill{
			target: t,
			root:   t.root,
			failed: newTargetError(CodeUnsupported, UntranslatableMessage(t.agent), t.root, 0, errors.ErrUnsupported),
		}, nil
	}
	authority, err := NewResourceAuthority(ctx, t.root)
	if err != nil {
		failed, invalid := skillAuthorityFailure(err)
		if invalid != nil {
			return preparedSkill{}, invalid
		}
		return preparedSkill{target: t, root: t.root, failed: failed}, nil
	}
	root := authority.destination
	installation, err := NewInstallationTarget(st, map[string]string{"destination": root}, skillRouting(t, root))
	if err != nil {
		return preparedSkill{}, err
	}
	return preparedSkill{target: t, root: root, installation: installation}, nil
}

func validateSkillReferences(ctx context.Context, rendered *RenderedBundle, t *InstallationTarget, bundle *Bundle) (*TargetError, error) {
	_, built := references(ctx, rendered, nil, t, bundle, nil)
	switch {
	case built == nil:
		return nil, nil
	case built.target != nil:
		return built.target, nil
	case built.overlap:
		return invalidState(built.value), nil
	}
	return nil, built.value
}

func prepareSkills(ctx context.Context, b *Bundle, identifier string, targets []Target, st store.Store) ([]preparedSkill, error) {
	if err := requireContext(ctx); err != nil {
		return nil, err
	}
	if err := validateBundleID(identifier); err != nil {
		return nil, err
	}
	if b != nil {
		if err := validateSkillBundle(b); err != nil {
			return nil, err
		}
	}
	if len(targets) == 0 {
		return nil, newValueError("at least one target is required")
	}
	for _, t := range targets {
		if err := checkSkillTarget(t); err != nil {
			return nil, err
		}
	}
	if err := checkStore(st); err != nil {
		return nil, err
	}
	prepared := make([]preparedSkill, 0, len(targets))
	type knownRoot struct {
		root  string
		index int
	}
	var known []knownRoot
	for _, t := range targets {
		item, err := prepareTarget(ctx, t, st)
		if err != nil {
			return nil, err
		}
		if item.failed == nil {
			if at := slices.IndexFunc(known, func(k knownRoot) bool { return pathsEqual(k.root, item.root) }); at >= 0 {
				item.isAlias, item.aliasOf = true, known[at].index
			}
		}
		switch {
		case item.isAlias:
			item.rendered = prepared[item.aliasOf].rendered
		case item.failed == nil:
			known = append(known, knownRoot{root: item.root, index: len(prepared)})
			if b != nil {
				if err := renderPrepared(ctx, b, &item); err != nil {
					return nil, err
				}
			}
		}
		prepared = append(prepared, item)
	}
	return prepared, nil
}

func renderPrepared(ctx context.Context, b *Bundle, item *preparedSkill) error {
	rendered, err := renderSkillBundleAt(b, item.root)
	if err != nil {
		failed, invalid := skillAuthorityFailure(asAuthorityError(err))
		if invalid != nil {
			return invalid
		}
		item.installation, item.failed = nil, failed
		return nil
	}
	failed, err := validateSkillReferences(ctx, rendered, item.installation, b)
	if err != nil {
		return err
	}
	if failed != nil {
		item.installation, item.failed = nil, failed
		return nil
	}
	item.rendered = rendered
	return nil
}

func unknownObservation(failed *TargetError) Observation {
	return Observation{State: StateUnknown, Err: failed}
}

func boolPointer(value bool) *bool {
	return &value
}

func skillObservation(ctx context.Context, observed InstallationObservation, b *Bundle, rendered *RenderedBundle) (Observation, error) {
	installations, metadataFailure, err := installedInventory(ctx, observed)
	if err != nil {
		return Observation{}, err
	}
	var installed *Installation
	if at := slices.IndexFunc(installations, func(item Installation) bool { return item.BundleID == observed.BundleID }); at >= 0 {
		copied := installations[at]
		copied.Entries = slices.Clone(copied.Entries)
		installed = &copied
	}
	var modifications []Modification
	var recoveryPaths []string
	var resourceFailure *TargetError
	for _, resource := range observed.Resources {
		for _, claim := range resource.Claims {
			if claim.Status != EditCurrent {
				modifications = append(modifications, Modification{BundleID: claim.BundleID, Path: claim.ArtifactID, Kind: ModificationContentChanged})
			}
		}
		recoveryPaths = append(recoveryPaths, resource.RecoveryPaths...)
		if resourceFailure == nil {
			resourceFailure = resource.Err
		}
	}
	failed := observed.Err
	if failed == nil {
		failed = metadataFailure
	}
	if failed == nil {
		failed = resourceFailure
	}
	observation := Observation{
		Installed:     installed,
		Installations: installations,
		Modifications: modifications,
		Err:           failed,
		RecoveryPaths: recoveryPaths,
	}
	switch {
	case len(recoveryPaths) > 0:
		observation.State = StateRecoveryNeeded
	case failed != nil:
		observation.State = StateUnknown
	case installed != nil:
		observation.State = StateInstalled
	default:
		observation.State = StateAbsent
	}
	if b != nil && installed != nil {
		observation.VersionMatches = boolPointer(installed.Version == b.Version())
		observation.RecordedContentMatches = boolPointer(installed.ContentDigest == b.ContentDigest())
	}
	if b != nil {
		observation.ContentMatches = boolPointer(observed.Current && observed.BundleDigest == b.ContentDigest() &&
			(rendered == nil || observed.RenderDigest == rendered.ContentDigest()))
	}
	return observation, nil
}

func installedInventory(ctx context.Context, observed InstallationObservation) ([]Installation, *TargetError, error) {
	var installations []Installation
	var metadataFailure *TargetError
	for _, resource := range observed.Resources {
		stored, ok, failed := readRecord(ctx, observed.Target.store, resourceKey(resource.Destination, "receipt"), "Receipt", receiptFromState)
		if failed != nil {
			metadataFailure = failed
			continue
		}
		if !ok {
			continue
		}
		var identifiers []string
		for _, claim := range stored.claims {
			if !slices.Contains(identifiers, claim.bundleID) {
				identifiers = append(identifiers, claim.bundleID)
			}
		}
		slices.Sort(identifiers)
		for _, identifier := range identifiers {
			installation, err := recordedInstallation(stored, identifier)
			if err != nil {
				return nil, nil, err
			}
			installations = append(installations, installation)
		}
	}
	return installations, metadataFailure, nil
}

func recordedInstallation(stored receipt, identifier string) (Installation, error) {
	var claims []ownedClaim
	for _, claim := range stored.claims {
		if claim.bundleID == identifier {
			claims = append(claims, claim)
		}
	}
	var entries []BundleEntry
	for _, claim := range claims {
		selector, subtree := claim.claim.selector.(SubtreeSelector)
		if claim.installed == nil || !subtree {
			continue
		}
		for _, item := range claim.installed.nodes {
			name := string(selector)
			if item.path != "" {
				name += "/" + item.path
			}
			mode, known := item.mode.uint64()
			entries = append(entries, BundleEntry{
				Path:       name,
				Data:       item.data,
				Dir:        !item.hasData,
				Executable: item.hasData && known && mode&0o111 != 0,
			})
		}
	}
	listed, err := inventory(entries)
	if err != nil {
		return Installation{}, err
	}
	return Installation{
		BundleID:      identifier,
		Version:       claims[0].version,
		ContentDigest: claims[0].bundleDigest,
		TransactionID: stored.transactionID,
		Entries:       listed,
	}, nil
}

// Inspect observes b's skill installation at each target, in request order, without changing anything.
func Inspect(ctx context.Context, b *Bundle, targets []Target, st store.Store) ([]TargetInspection, error) {
	if b == nil {
		return nil, newTypeError("inspect requires a bundle snapshot")
	}
	prepared, err := prepareSkills(ctx, b, b.ID(), targets, st)
	if err != nil {
		return nil, err
	}
	result := make([]TargetInspection, 0, len(prepared))
	for _, item := range prepared {
		if item.isAlias && item.aliasOf < len(result) {
			aliased := result[item.aliasOf]
			aliased.Target, aliased.IsAlias, aliased.AliasOf = item.target, true, item.aliasOf
			aliased.Observation = cloneObservation(aliased.Observation)
			result = append(result, aliased)
			continue
		}
		var observation Observation
		switch {
		case item.failed != nil:
			observation = unknownObservation(item.failed)
		case item.rendered == nil:
			return nil, newValueError("skill inspection requires rendered content")
		default:
			observation, err = inspectSkill(ctx, b, item)
			if err != nil {
				return nil, err
			}
		}
		result = append(result, TargetInspection{Target: item.target, Root: item.root, Observation: observation})
	}
	return result, nil
}

func inspectSkill(ctx context.Context, b *Bundle, item preparedSkill) (Observation, error) {
	observed, err := InspectInstallation(ctx, b.ID(), item.installation)
	if err != nil {
		return Observation{}, err
	}
	observation, err := skillObservation(ctx, observed, b, item.rendered)
	if err != nil {
		return Observation{}, err
	}
	planned, err := Preview(ctx, b, item.rendered, item.installation, PreviewOptions{Acquisition: AcquireConflict})
	if err != nil {
		return Observation{}, err
	}
	var conflicts []Conflict
	var planFailure *TargetError
	for _, plan := range planned.resources {
		switch {
		case plan.err == nil:
		case plan.err.Code == CodeConflict:
			conflicts = append(conflicts, Conflict{Path: plan.resource.Destination})
		case planFailure == nil:
			planFailure = plan.Err()
		}
	}
	failed := observation.Err
	if failed == nil {
		failed = planned.Err()
	}
	if failed == nil {
		failed = planFailure
	}
	if failed != nil && observation.State != StateRecoveryNeeded {
		observation.State = StateUnknown
	}
	observation.Conflicts = conflicts
	observation.Err = failed
	return observation, nil
}

func cloneObservation(o Observation) Observation {
	if o.Installed != nil {
		installed := *o.Installed
		installed.Entries = slices.Clone(installed.Entries)
		o.Installed = &installed
	}
	installations := slices.Clone(o.Installations)
	for index := range installations {
		installations[index].Entries = slices.Clone(installations[index].Entries)
	}
	o.Installations = installations
	o.Modifications = slices.Clone(o.Modifications)
	o.Conflicts = slices.Clone(o.Conflicts)
	o.RecoveryPaths = slices.Clone(o.RecoveryPaths)
	o.VersionMatches = cloneBool(o.VersionMatches)
	o.RecordedContentMatches = cloneBool(o.RecordedContentMatches)
	o.ContentMatches = cloneBool(o.ContentMatches)
	o.Err = copyTargetError(o.Err)
	return o
}

func cloneBool(value *bool) *bool {
	if value == nil {
		return nil
	}
	copied := *value
	return &copied
}
