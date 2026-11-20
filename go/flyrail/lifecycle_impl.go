package flyrail

import (
	"bytes"
	"context"
	"errors"
	"slices"
	"strings"
	"time"

	"github.com/nielsmadan/flyrail/go/internal/native"
	"github.com/nielsmadan/flyrail/go/internal/pathx"
	"github.com/nielsmadan/flyrail/go/internal/unicodex"
)

type buildError struct {
	value   *ValueError
	overlap bool
	target  *TargetError
}

func invalidBuild(err error) *buildError {
	if failure, ok := errors.AsType[*ValueError](err); ok {
		return &buildError{value: failure}
	}
	return &buildError{value: newValueError(err.Error())}
}

func invalidRequest(message string) *buildError {
	return &buildError{value: newValueError(message)}
}

func targetBuild(failed *TargetError) *buildError {
	return &buildError{target: failed}
}

func (e *buildError) failure() *TargetError {
	if e.target != nil {
		return e.target
	}
	return invalidState(e.value)
}

func invalidState(err *ValueError) *TargetError {
	return newTargetError(CodeInvalidState, err.Message, "", 0, err)
}

func refKey(reference ResourceRef) string {
	return pathKey(reference.Destination) + "\x00" + string(reference.Kind)
}

func installationKey(identifier string, t *InstallationTarget) string {
	return indexKey(identifier, t.ContextDigest())
}

func installationIndex(ctx context.Context, identifier string, t *InstallationTarget) (*index, *TargetError) {
	recorded, ok, failed := readRecord(ctx, t.store, installationKey(identifier, t), "Index", indexFromState)
	if failed != nil || !ok {
		return nil, failed
	}
	if recorded.bundleID != identifier || recorded.contextDigest != t.ContextDigest() {
		return nil, stateFailure(t.store, CodeInvalidState, "installation index context disagrees")
	}
	return &recorded, nil
}

func sameOptionalIndex(a, b *index) bool {
	if a == nil || b == nil {
		return a == b
	}
	return sameRecord(*a, *b)
}

func sameOptionalGeneration(a, b *generation) bool {
	if a == nil || b == nil {
		return a == b
	}
	return sameRecord(*a, *b)
}

func missingReceipt(reference ResourceRef) *TargetError {
	return failure(CodeInvalidState, "recorded ownership receipt is missing; restore metadata before retrying", reference.Destination)
}

func checkReceipt(ctx context.Context, t *InstallationTarget, recorded *index, reference ResourceRef) *TargetError {
	if !expectsReceipt(recorded, reference) {
		return nil
	}
	_, ok, failed := readRecord(ctx, t.store, receiptKey(reference), "Receipt", receiptFromState)
	if failed != nil {
		return failed
	}
	if !ok {
		return missingReceipt(reference)
	}
	return nil
}

func portableParts(path string) []string {
	components := pathx.Components(path)
	parts := make([]string, 0, len(components))
	for _, component := range components {
		parts = append(parts, portablePathKey(unicodex.Lossy(component)))
	}
	return parts
}

func overlapsManagement(path, destination string) bool {
	parentPath, ok := pathx.Parent(destination)
	if !ok {
		parentPath = destination
	}
	parent, selected := portableParts(parentPath), portableParts(path)
	if len(selected) <= len(parent) {
		return slices.Equal(parent[:len(selected)], selected)
	}
	return slices.Equal(selected[:len(parent)], parent) && native.ManagementName(unicodex.Lossy(pathx.Components(path)[len(parent)]))
}

func pathOverlap(left, right string) bool {
	first, second := portableParts(left), portableParts(right)
	shared := min(len(first), len(second))
	return slices.Equal(first[:shared], second[:shared])
}

func storePaths(t *InstallationTarget) ([]string, *TargetError) {
	var paths []string
	if location, ok := t.store.Location(); ok {
		paths = append(paths, location)
	}
	if staging, ok := t.store.StagingDirectory(); ok {
		paths = append(paths, staging)
	}
	physical := make([]string, 0, len(paths))
	for _, path := range paths {
		resolved, err := native.PhysicalLocation(path)
		if err != nil {
			return nil, errorDetail(nativeAuthorityError(err))
		}
		physical = append(physical, resolved)
	}
	return physical, nil
}

func resolveArtifacts(ctx context.Context, rendered *RenderedBundle) (map[string]ResourceRef, *TargetError) {
	resolved := make(map[string]ResourceRef, len(rendered.artifacts))
	for _, artifact := range rendered.artifacts {
		reference, failed := resourceOf(ctx, artifact)
		if failed != nil {
			return nil, failed
		}
		resolved[artifact.id] = reference
	}
	return resolved, nil
}

func references(ctx context.Context, rendered *RenderedBundle, recorded *index, t *InstallationTarget, bundle *Bundle, resolved map[string]ResourceRef) ([]ResourceRef, *buildError) {
	stores, failed := storePaths(t)
	if failed != nil {
		return nil, targetBuild(failed)
	}
	sources := bundle.SourceRoots()
	for _, source := range sources {
		if slices.ContainsFunc(stores, func(location string) bool { return pathOverlap(source, location) }) {
			return nil, invalidRequest("state store overlaps bundle source")
		}
	}
	if resolved == nil {
		if resolved, failed = resolveArtifacts(ctx, rendered); failed != nil {
			return nil, targetBuild(failed)
		}
	}
	outputs := map[string][]string{}
	refs := make([]ResourceRef, 0, len(rendered.artifacts))
	for _, artifact := range rendered.artifacts {
		reference, ok := resolved[artifact.id]
		if !ok {
			return nil, invalidRequest("rendered artifact reference is missing")
		}
		output := reference.Destination
		if artifact.subtree != "" {
			output = pathx.Join(reference.Destination, artifact.subtree)
		}
		outputs[refKey(reference)] = append(outputs[refKey(reference)], output)
		refs = append(refs, reference)
	}
	if recorded != nil {
		refs = append(refs, recorded.resources()...)
	}
	refs = slices.CompactFunc(destinationOrder(refs), resourceRefsEqual)
	destinations := make(map[string]bool, len(refs))
	for _, reference := range refs {
		if destinations[pathKey(reference.Destination)] {
			return nil, invalidRequest("resource kinds disagree at the same destination")
		}
		destinations[pathKey(reference.Destination)] = true
	}
	for _, reference := range refs {
		if slices.ContainsFunc(stores, func(location string) bool {
			return pathOverlap(reference.Destination, location) || overlapsManagement(location, reference.Destination)
		}) {
			return nil, &buildError{value: newValueError("state store overlaps a resource or authority"), overlap: true}
		}
		for _, source := range sources {
			if overlapsManagement(source, reference.Destination) {
				return nil, invalidRequest("resource authority overlaps bundle source")
			}
			if slices.ContainsFunc(outputs[refKey(reference)], func(output string) bool { return pathOverlap(source, output) }) {
				return nil, invalidRequest("resource output overlaps bundle source")
			}
		}
	}
	return refs, nil
}

type edge struct {
	from ResourceRef
	to   ResourceRef
}

func findClaim(claims []ownedClaim, claimID string) *ownedClaim {
	index := slices.IndexFunc(claims, func(item ownedClaim) bool { return item.claimID == claimID })
	if index < 0 {
		return nil
	}
	return &claims[index]
}

func containsRequirement(items []requirement, wanted requirement) bool {
	return slices.ContainsFunc(items, func(item requirement) bool { return item.key() == wanted.key() })
}

func previousClaims(plan ResourcePlan) []ownedClaim {
	if plan.previous == nil {
		return nil
	}
	return plan.previous.claims
}

func assetEdges(plans []ResourcePlan, plan ResourcePlan, refs map[string]ResourceRef) ([]edge, *ValueError) {
	var edges []edge
	for _, old := range previousClaims(plan) {
		next := findClaim(plan.receipt.claims, old.claimID)
		for _, required := range old.requirements {
			resource, ok := refs[pathKey(required.path)]
			if !ok || resourceRefsEqual(resource, plan.resource) {
				continue
			}
			index := slices.IndexFunc(plans, func(item ResourcePlan) bool { return resourceRefsEqual(item.resource, resource) })
			if index < 0 {
				return nil, newValueError("planned dependency resource is missing")
			}
			oldAsset := findClaim(previousClaims(plans[index]), required.identifier)
			newAsset := findClaim(plans[index].receipt.claims, required.identifier)
			if oldAsset != nil && newAsset != nil &&
				!optionalRevisionsEqual(oldAsset.installed, newAsset.installed) &&
				!containsRequirement(old.stableRequirements, required) {
				return nil, newValueError("referenced asset changes require a distinct revision destination")
			}
			if oldAsset != nil && newAsset == nil {
				if next != nil && containsRequirement(next.requirements, required) {
					return nil, newValueError("cannot retire an asset with an active reference")
				}
				edges = append(edges, edge{from: resource, to: plan.resource})
			}
		}
	}
	return edges, nil
}

func orderPlans(plans []ResourcePlan, rendered *RenderedBundle, byID map[string]ResourceRef) ([]ResourcePlan, *ValueError) {
	refs := make(map[string]ResourceRef, len(plans))
	for _, plan := range plans {
		refs[pathKey(plan.resource.Destination)] = plan.resource
	}
	var edges []edge
	for _, dependency := range rendered.dependencies {
		dependent, okDependent := byID[dependency.dependent]
		required, okRequired := byID[dependency.required]
		if !okDependent || !okRequired {
			return nil, newValueError("planned artifact reference is missing")
		}
		if !resourceRefsEqual(dependent, required) {
			edges = append(edges, edge{from: dependent, to: required})
		}
	}
	for _, plan := range plans {
		found, failed := assetEdges(plans, plan, refs)
		if failed != nil {
			return nil, failed
		}
		edges = append(edges, found...)
	}
	pending := make(map[string]ResourcePlan, len(plans))
	for _, plan := range plans {
		pending[refKey(plan.resource)] = plan
	}
	ordered := make([]ResourcePlan, 0, len(plans))
	for len(pending) > 0 {
		var ready []ResourceRef
		for _, plan := range pending {
			if !slices.ContainsFunc(edges, func(item edge) bool {
				_, waiting := pending[refKey(item.to)]
				return resourceRefsEqual(item.from, plan.resource) && waiting
			}) {
				ready = append(ready, plan.resource)
			}
		}
		if len(ready) == 0 {
			return nil, newValueError("cyclic resource dependencies")
		}
		for _, reference := range destinationOrder(ready) {
			ordered = append(ordered, pending[refKey(reference)])
			delete(pending, refKey(reference))
		}
	}
	return ordered, nil
}

type requirementSets map[string][]requirement

func (s requirementSets) add(artifactID string, item requirement) bool {
	items, ok := s[artifactID]
	if !ok {
		return false
	}
	if !containsRequirement(items, item) {
		s[artifactID] = append(items, item)
	}
	return true
}

func sameRequirementSet(a, b []requirement) bool {
	return len(a) == len(b) && !slices.ContainsFunc(a, func(item requirement) bool { return !containsRequirement(b, item) })
}

func sortedRequirements(sets requirementSets) map[string][]requirement {
	result := make(map[string][]requirement, len(sets))
	for artifactID, items := range sets {
		sorted := slices.Clone(items)
		slices.SortFunc(sorted, func(a, b requirement) int {
			if order := comparePaths(a.path, b.path); order != 0 {
				return order
			}
			return strings.Compare(a.identifier, b.identifier)
		})
		result[artifactID] = sorted
	}
	return result
}

func artifactRequirementSets(rendered *RenderedBundle, resolved map[string]ResourceRef) (requirements, stable requirementSets, failed *buildError) {
	byArtifact := make(map[string]RenderedArtifact, len(rendered.artifacts))
	requirements, stable = requirementSets{}, requirementSets{}
	for _, artifact := range rendered.artifacts {
		byArtifact[artifact.id] = artifact
		requirements[artifact.id] = nil
		stable[artifact.id] = nil
	}
	for _, dependency := range rendered.dependencies {
		required, ok := resolved[dependency.required]
		if !ok {
			return nil, nil, invalidRequest("planned dependency reference is missing")
		}
		artifact, ok := byArtifact[dependency.required]
		if !ok {
			return nil, nil, invalidRequest("planned dependency artifact is missing")
		}
		claimID, err := artifactClaim(artifact).identity(required.Destination)
		if err != nil {
			return nil, nil, invalidBuild(err)
		}
		pair := requirement{path: required.Destination, identifier: claimID}
		if !requirements.add(dependency.dependent, pair) {
			return nil, nil, invalidRequest("planned artifact requirements are missing")
		}
		if dependency.mode == DependencyStableReference && !stable.add(dependency.dependent, pair) {
			return nil, nil, invalidRequest("planned artifact requirements are missing")
		}
	}
	return requirements, stable, nil
}

type claimGroup struct {
	reference ResourceRef
	claim     Claim
	artifact  RenderedArtifact
}

func groupArtifacts(rendered *RenderedBundle, resolved map[string]ResourceRef, requirements, stable requirementSets) (map[string][]RenderedArtifact, *buildError) {
	var groups []claimGroup
	for _, artifact := range rendered.artifacts {
		reference, ok := resolved[artifact.id]
		if !ok {
			return nil, invalidRequest("planned artifact reference is missing")
		}
		claim := artifactClaim(artifact)
		index := slices.IndexFunc(groups, func(group claimGroup) bool {
			return resourceRefsEqual(group.reference, reference) && group.claim.Equal(claim)
		})
		if index < 0 {
			groups = append(groups, claimGroup{reference: reference, claim: claim, artifact: artifact})
			continue
		}
		previous := groups[index].artifact
		if !contentEqual(previous.content, artifact.content) || previous.family != artifact.family ||
			!schemasEqual(previous.schemaRequirements, artifact.schemaRequirements) {
			return nil, invalidRequest("physical aliases have incompatible desired content")
		}
		if !sameRequirementSet(stable[previous.id], stable[artifact.id]) {
			return nil, invalidRequest("physical aliases have incompatible stable-reference requirements")
		}
		aliases, ok := requirements[artifact.id]
		if !ok {
			return nil, invalidRequest("planned artifact requirements are missing")
		}
		for _, item := range aliases {
			if !requirements.add(previous.id, item) {
				return nil, invalidRequest("planned artifact requirements are missing")
			}
		}
		if artifact.requireExisting && !previous.requireExisting {
			previous.requireExisting = true
			groups[index].artifact = previous
		}
	}
	selected := map[string][]RenderedArtifact{}
	for _, group := range groups {
		selected[refKey(group.reference)] = append(selected[refKey(group.reference)], group.artifact)
	}
	for _, artifacts := range selected {
		slices.SortFunc(artifacts, func(a, b RenderedArtifact) int { return strings.Compare(a.id, b.id) })
	}
	return selected, nil
}

func plannedGeneration(bundle *Bundle, rendered *RenderedBundle, refs []ResourceRef, selected map[string][]RenderedArtifact) (*generation, *buildError) {
	if bundle == nil {
		return nil, nil
	}
	var members []membership
	for _, reference := range refs {
		artifacts, ok := selected[refKey(reference)]
		if !ok {
			continue
		}
		claims := make([]string, 0, len(artifacts))
		for _, artifact := range artifacts {
			identity, err := artifactClaim(artifact).identity(reference.Destination)
			if err != nil {
				return nil, invalidBuild(err)
			}
			claims = append(claims, identity)
		}
		slices.Sort(claims)
		members = append(members, membership{resource: reference, claimIDs: claims})
	}
	planned, err := newGeneration(bundle.Version(), bundle.ContentDigest(), rendered.ContentDigest(), members)
	if err != nil {
		return nil, invalidBuild(err)
	}
	return &planned, nil
}

func unsupportedRendering() *TargetError {
	return newTargetError(CodeUnsupported, "rendered target has unsupported requirements", "", 0, nil)
}

type previewRequest struct {
	identifier      string
	bundle          *Bundle
	rendered        *RenderedBundle
	target          *InstallationTarget
	acquisition     Acquisition
	replaceModified bool
}

func (r previewRequest) empty(failed *TargetError) *LifecyclePreview {
	return &LifecyclePreview{
		bundleID:        r.identifier,
		bundle:          r.bundle,
		rendered:        r.rendered,
		target:          r.target,
		acquisition:     r.acquisition,
		replaceModified: r.replaceModified,
		err:             failed,
	}
}

func buildPreview(ctx context.Context, r previewRequest) (*LifecyclePreview, *buildError) {
	t := r.target
	recorded, failed := installationIndex(ctx, r.identifier, t)
	if failed != nil {
		return nil, targetBuild(failed)
	}
	resolved, failed := resolveArtifacts(ctx, r.rendered)
	if failed != nil {
		return nil, targetBuild(failed)
	}
	refs, built := references(ctx, r.rendered, recorded, t, r.bundle, resolved)
	if built != nil {
		return nil, built
	}
	requirements, stable, built := artifactRequirementSets(r.rendered, resolved)
	if built != nil {
		return nil, built
	}
	selected, built := groupArtifacts(r.rendered, resolved, requirements, stable)
	if built != nil {
		return nil, built
	}
	normalized, normalizedStable := sortedRequirements(requirements), sortedRequirements(stable)
	plans := make([]ResourcePlan, 0, len(refs))
	for _, reference := range refs {
		plan, failed := planResource(ctx, t.store, planInputs{
			reference:          reference,
			identifier:         r.identifier,
			bundle:             r.bundle,
			rendered:           r.rendered,
			artifacts:          selected[refKey(reference)],
			acquisition:        r.acquisition,
			replaceModified:    r.replaceModified,
			requirements:       normalized,
			stableRequirements: normalizedStable,
		})
		if failed != nil {
			return nil, targetBuild(failed)
		}
		if plan.previous == nil && expectsReceipt(recorded, reference) {
			plan = plan.withError(missingReceipt(reference))
		}
		plans = append(plans, plan)
	}
	if !slices.ContainsFunc(plans, func(plan ResourcePlan) bool { return plan.err != nil }) {
		ordered, invalid := orderPlans(plans, r.rendered, resolved)
		if invalid != nil {
			return nil, &buildError{value: invalid}
		}
		plans = ordered
	}
	planned, built := plannedGeneration(r.bundle, r.rendered, refs, selected)
	if built != nil {
		return nil, built
	}
	preview := r.empty(nil)
	preview.index = recorded
	preview.generation = planned
	preview.resources = plans
	if !r.rendered.Supported() {
		preview.err = unsupportedRendering()
	}
	return preview, nil
}

func publicPreview(ctx context.Context, r previewRequest) (*LifecyclePreview, error) {
	preview, built := buildPreview(ctx, r)
	switch {
	case built == nil:
		return preview, nil
	case built.target != nil:
		return r.empty(built.target), nil
	}
	return nil, built.value
}

func observedResource(ctx context.Context, t *InstallationTarget, reference ResourceRef) (ResourceObservation, *TargetError) {
	observed, failed := summarizeResource(ctx, t.store, reference)
	if failed != nil {
		return ResourceObservation{}, failed
	}
	return ResourceObservation{
		Destination:    observed.destination,
		Claims:         observed.claims,
		RevisionDigest: observed.contentDigest,
		RecoveryPaths:  observed.recoveryPaths,
	}, nil
}

func inspectResource(ctx context.Context, t *InstallationTarget, recorded *index, reference ResourceRef) ResourceObservation {
	failed := checkReceipt(ctx, t, recorded, reference)
	if failed == nil {
		var item ResourceObservation
		if item, failed = observedResource(ctx, t, reference); failed == nil {
			return item
		}
	}
	return ResourceObservation{Destination: reference.Destination, Err: failed}
}

func claimKey(destination, claimID string) string {
	return pathKey(destination) + "\x01" + claimID
}

func intactResource(resource ResourceObservation, bundleID string, current *generation) bool {
	return resource.Err == nil && len(resource.RecoveryPaths) == 0 &&
		!slices.ContainsFunc(resource.Claims, func(claim ClaimObservation) bool {
			return claim.BundleID == bundleID && (current == nil ||
				claim.Version != current.version ||
				claim.BundleDigest != current.bundleDigest ||
				claim.RenderDigest != current.renderDigest ||
				claim.Status != EditCurrent)
		})
}

func inspect(ctx context.Context, bundleID string, t *InstallationTarget) InstallationObservation {
	observation := InstallationObservation{BundleID: bundleID, Target: t}
	recorded, failed := installationIndex(ctx, bundleID, t)
	if failed != nil {
		observation.Err = failed
		return observation
	}
	var references []ResourceRef
	if recorded != nil {
		references = recorded.resources()
	}
	for _, reference := range references {
		observation.Resources = append(observation.Resources, inspectResource(ctx, t, recorded, reference))
	}
	var current *generation
	if recorded != nil {
		current = recorded.current
		observation.Pending = recorded.pending != nil || recorded.previous != nil || len(recorded.residual) > 0
	}
	actual := map[string]bool{}
	for _, resource := range observation.Resources {
		for _, claim := range resource.Claims {
			if claim.BundleID == bundleID {
				actual[claimKey(resource.Destination, claim.ClaimID)] = true
			}
		}
	}
	expected := map[string]bool{}
	if current != nil {
		for _, member := range current.membership {
			for _, claimID := range member.claimIDs {
				expected[claimKey(member.resource.Destination, claimID)] = true
			}
		}
		observation.Version = current.version
		observation.BundleDigest = current.bundleDigest
		observation.RenderDigest = current.renderDigest
	}
	intact := !slices.ContainsFunc(observation.Resources, func(resource ResourceObservation) bool {
		return !intactResource(resource, bundleID, current)
	})
	observation.Current = current != nil && !observation.Pending && intact && sameKeys(actual, expected)
	return observation
}

func sameKeys(a, b map[string]bool) bool {
	if len(a) != len(b) {
		return false
	}
	for key := range a {
		if !b[key] {
			return false
		}
	}
	return true
}

func approvedAncestors(plan ResourcePlan, created []ancestor, destination, staging string) []ancestor {
	approved := slices.Clone(plan.ancestors)
	for _, item := range created {
		if (pathWithin(destination, item.path) || pathWithin(staging, item.path)) &&
			!slices.ContainsFunc(plan.ancestors, item.equal) {
			approved = append(approved, item)
		}
	}
	return approved
}

func recordPresent(ctx context.Context, t *InstallationTarget, key string) (bool, *TargetError) {
	_, present, err := t.store.Read(ctx, key)
	if err != nil {
		return false, storeFailure(err)
	}
	return present, nil
}

func freshPlan(ctx context.Context, t *InstallationTarget, plan ResourcePlan, created []ancestor) (ResourcePlan, *TargetError) {
	destination := plan.resource.Destination
	if failed := require(destination, plan.before); failed != nil {
		return ResourcePlan{}, failed
	}
	stored, present, err := t.store.Read(ctx, receiptKey(plan.resource))
	if err != nil {
		return ResourcePlan{}, storeFailure(err)
	}
	if present != (plan.previous != nil) || plan.previous != nil && !bytes.Equal(stored, encodeRecord(*plan.previous)) {
		return ResourcePlan{}, failure(CodeConcurrentChange, "preview receipt changed", destination)
	}
	recorded := false
	for _, key := range []string{preparationKey(plan.resource), journalKey(plan.resource)} {
		present, failed := recordPresent(ctx, t, key)
		if failed != nil {
			return ResourcePlan{}, failed
		}
		recorded = recorded || present
	}
	if recorded == (len(plan.recoveryPaths) == 0) {
		return ResourcePlan{}, failure(CodeConcurrentChange, "preview recovery changed", destination)
	}
	staging := pathx.Join(plan.transactionDirectory, "staging")
	current, failed := resourceAncestors(destination, staging)
	if failed != nil {
		return ResourcePlan{}, failed
	}
	approved := approvedAncestors(plan, created, destination, staging)
	same := !slices.ContainsFunc(current, func(item ancestor) bool { return !slices.ContainsFunc(approved, item.equal) }) &&
		!slices.ContainsFunc(approved, func(item ancestor) bool { return !slices.ContainsFunc(current, item.equal) })
	if !same {
		return ResourcePlan{}, failure(CodeConcurrentChange, "preview ancestor changed", staging)
	}
	return plan.withAncestors(current), nil
}

func fresh(ctx context.Context, preview *LifecyclePreview, created []ancestor) (*LifecyclePreview, *TargetError) {
	for _, observed := range preview.readPreconditions {
		if failed := observed.require(); failed != nil {
			return nil, failed
		}
	}
	t := preview.target
	if failed := requireIndex(ctx, t.store, installationKey(preview.bundleID, t), preview.index); failed != nil {
		return nil, failed
	}
	resources := make([]ResourcePlan, 0, len(preview.resources))
	for _, plan := range preview.resources {
		refreshed, failed := freshPlan(ctx, t, plan, created)
		if failed != nil {
			return nil, failed
		}
		resources = append(resources, refreshed)
	}
	return preview.withResources(resources), nil
}

func failedResult(ctx context.Context, identifier string, t *InstallationTarget, failed *TargetError, notices []Notice) InstallationResult {
	return InstallationResult{
		Status:      StatusFailed,
		Observation: inspect(context.WithoutCancel(ctx), identifier, t),
		Err:         failed,
		Notices:     notices,
	}
}

func recoveryPaths(ctx context.Context, t *InstallationTarget, reference ResourceRef, directory string) []string {
	paths, failed := pendingRecovery(ctx, t.store, reference)
	if failed == nil {
		return paths
	}
	if directory == "" {
		return nil
	}
	prepared, ok, failed := readRecord(ctx, t.store, preparationKey(reference), "Preparation", preparationFromState)
	if failed == nil && ok && pathsEqual(prepared.directory, directory) {
		return []string{directory}
	}
	return nil
}

func acquireLocks(ctx context.Context, identifier string, t *InstallationTarget, expected *index, refs []ResourceRef, timeout time.Duration) (*transactionLocks, *TargetError) {
	return acquireTransactionLocks(ctx, t.store, installationKey(identifier, t), refs, timeout, func() *TargetError {
		current, failed := installationIndex(ctx, identifier, t)
		if failed != nil {
			return failed
		}
		if !sameOptionalIndex(current, expected) {
			return stateFailure(t.store, CodeConcurrentChange, "index changed while acquiring locks")
		}
		return nil
	})
}

func unchangedPlans(plans []ResourcePlan) bool {
	return !slices.ContainsFunc(plans, func(plan ResourcePlan) bool {
		return !plan.before.equal(plan.after) || plan.previous == nil || !sameRecord(*plan.previous, plan.receipt)
	})
}

func planResources(plans []ResourcePlan) []ResourceRef {
	refs := make([]ResourceRef, 0, len(plans))
	for _, plan := range plans {
		refs = append(refs, plan.resource)
	}
	return refs
}

func containsRef(refs []ResourceRef, reference ResourceRef) bool {
	return slices.ContainsFunc(refs, func(item ResourceRef) bool { return resourceRefsEqual(item, reference) })
}

func resourceResults(ctx context.Context, t *InstallationTarget, plans []ResourcePlan, outcome transactionOutcome) []ResourceResult {
	var results []ResourceResult
	for _, plan := range plans {
		switch {
		case outcome.failed != nil && resourceRefsEqual(*outcome.failed, plan.resource):
			paths := recoveryPaths(ctx, t, plan.resource, plan.transactionDirectory)
			status := StatusFailed
			switch {
			case containsRef(outcome.completed, plan.resource):
				status = StatusApplied
			case len(paths) > 0:
				status = StatusIncomplete
			}
			return append(results, ResourceResult{Resource: plan.resource, Status: status, Err: outcome.err, RecoveryPaths: paths})
		case containsRef(outcome.completed, plan.resource):
			results = append(results, ResourceResult{Resource: plan.resource, Status: StatusApplied})
		case containsRef(outcome.unchanged, plan.resource):
			results = append(results, ResourceResult{Resource: plan.resource, Status: StatusUnchanged})
		}
	}
	return results
}

func firstPlanError(preview *LifecyclePreview) *TargetError {
	if preview.err != nil {
		return preview.err
	}
	for _, plan := range preview.resources {
		if plan.err != nil {
			return plan.err
		}
	}
	return nil
}

func runLocked(ctx context.Context, preview *LifecyclePreview, locks *transactionLocks) InstallationResult {
	identifier, t := preview.bundleID, preview.target
	if failed := firstPlanError(preview); failed != nil {
		return failedResult(ctx, identifier, t, failed, nil)
	}
	if slices.ContainsFunc(preview.resources, func(plan ResourcePlan) bool { return len(plan.recoveryPaths) > 0 }) {
		return failedResult(ctx, identifier, t, failure(CodeRecoveryNeeded, "preview requires recovery and re-observation", ""), nil)
	}
	refreshed, failed := fresh(ctx, preview, locks.created)
	if failed != nil {
		return failedResult(ctx, identifier, t, failed, nil)
	}
	unchanged := unchangedPlans(refreshed.resources)
	settled := refreshed.index != nil && sameOptionalGeneration(refreshed.index.current, refreshed.generation) &&
		refreshed.index.pending == nil && len(refreshed.index.residual) == 0
	if unchanged && settled {
		return InstallationResult{Status: StatusUnchanged, Observation: inspect(ctx, identifier, t), Notices: refreshed.rendered.Notices()}
	}
	if unchanged && refreshed.index == nil && refreshed.bundle == nil {
		return InstallationResult{Status: StatusUnchanged, Observation: inspect(ctx, identifier, t)}
	}
	digest := t.ContextDigest()
	pending, err := pendingIndex(identifier, digest, refreshed.index, refreshed.generation, planResources(refreshed.resources))
	if err != nil {
		return failedResult(ctx, identifier, t, stateFailure(t.store, CodeInvalidState, err.Error()), nil)
	}
	completed, err := completedIndex(identifier, digest, refreshed.generation)
	if err != nil {
		return failedResult(ctx, identifier, t, stateFailure(t.store, CodeInvalidState, err.Error()), nil)
	}
	outcome, failed := executeTransaction(ctx, locks, refreshed.resources, refreshed.index, pending, completed, refreshed.readPreconditions...)
	ctx = context.WithoutCancel(ctx)
	if failed != nil {
		return failedResult(ctx, identifier, t, failed, nil)
	}
	results := resourceResults(ctx, t, refreshed.resources, outcome)
	status := StatusFailed
	switch {
	case outcome.err == nil:
		status = StatusApplied
	case len(outcome.completed) > 0:
		status = StatusPartial
	case slices.ContainsFunc(results, func(result ResourceResult) bool { return result.Status == StatusIncomplete }):
		status = StatusIncomplete
	}
	return InstallationResult{
		Status:      status,
		Observation: inspect(ctx, identifier, t),
		Resources:   results,
		Err:         outcome.err,
		Notices:     refreshed.rendered.Notices(),
	}
}

func apply(ctx context.Context, preview *LifecyclePreview, timeout time.Duration) InstallationResult {
	identifier, t := preview.bundleID, preview.target
	if preview.err != nil {
		return failedResult(ctx, identifier, t, preview.Err(), preview.rendered.Notices())
	}
	if !preview.rendered.Supported() {
		return failedResult(ctx, identifier, t, unsupportedRendering(), preview.rendered.Notices())
	}
	if _, failed := fresh(ctx, preview, nil); failed != nil {
		return failedResult(ctx, identifier, t, failed, nil)
	}
	locks, failed := acquireLocks(ctx, identifier, t, preview.index, planResources(preview.resources), timeout)
	if failed != nil {
		return failedResult(ctx, identifier, t, failed, nil)
	}
	defer func() { _ = locks.Unlock() }()
	return runLocked(ctx, preview, locks)
}

func recoverLocked(ctx context.Context, identifier string, t *InstallationTarget, recorded *index, refs []ResourceRef, locks *transactionLocks) InstallationResult {
	current, failed := installationIndex(ctx, identifier, t)
	if failed != nil {
		return failedResult(ctx, identifier, t, failed, nil)
	}
	if !sameOptionalIndex(current, recorded) {
		return failedResult(ctx, identifier, t, stateFailure(t.store, CodeConcurrentChange, "index changed while acquiring locks"), nil)
	}
	for _, reference := range refs {
		if failed := checkReceipt(ctx, t, recorded, reference); failed != nil {
			return failedResult(ctx, identifier, t, failed, nil)
		}
	}
	ctx = context.WithoutCancel(ctx)
	var results []ResourceResult
	changed := false
	for _, guard := range locks.resources {
		paths, failed := pendingRecovery(ctx, t.store, guard.reference)
		if failed == nil {
			failed = recoverResource(ctx, t.store, guard)
		}
		if failed != nil {
			results = append(results, ResourceResult{
				Resource:      guard.reference,
				Status:        StatusIncomplete,
				Err:           failed,
				RecoveryPaths: recoveryPaths(ctx, t, guard.reference, ""),
			})
			return InstallationResult{Status: StatusIncomplete, Observation: inspect(ctx, identifier, t), Resources: results, Err: failed}
		}
		status := StatusUnchanged
		if len(paths) > 0 {
			status, changed = StatusApplied, true
		}
		results = append(results, ResourceResult{Resource: guard.reference, Status: status})
	}
	status := StatusUnchanged
	if changed {
		status = StatusApplied
	}
	return InstallationResult{Status: status, Observation: inspect(ctx, identifier, t), Resources: results}
}

func recoverInstallation(ctx context.Context, identifier string, t *InstallationTarget, timeout time.Duration) InstallationResult {
	recorded, failed := installationIndex(ctx, identifier, t)
	if failed != nil {
		return failedResult(ctx, identifier, t, failed, nil)
	}
	if recorded == nil {
		return InstallationResult{Status: StatusUnchanged, Observation: inspect(ctx, identifier, t)}
	}
	refs, built := references(ctx, EmptyRenderedBundle(), recorded, t, nil, nil)
	if built != nil {
		return failedResult(ctx, identifier, t, built.failure(), nil)
	}
	locks, failed := acquireLocks(ctx, identifier, t, recorded, refs, timeout)
	if failed != nil {
		return failedResult(ctx, identifier, t, failed, nil)
	}
	defer func() { _ = locks.Unlock() }()
	return recoverLocked(ctx, identifier, t, recorded, refs, locks)
}

func synchronize(ctx context.Context, r previewRequest, timeout time.Duration) InstallationResult {
	identifier, t := r.identifier, r.target
	if !r.rendered.Supported() {
		return failedResult(ctx, identifier, t, unsupportedRendering(), r.rendered.Notices())
	}
	recorded, failed := installationIndex(ctx, identifier, t)
	if failed != nil {
		return failedResult(ctx, identifier, t, failed, nil)
	}
	refs, built := references(ctx, r.rendered, recorded, t, r.bundle, nil)
	if built != nil {
		return failedResult(ctx, identifier, t, built.failure(), nil)
	}
	locks, failed := acquireLocks(ctx, identifier, t, recorded, refs, timeout)
	if failed != nil {
		return failedResult(ctx, identifier, t, failed, nil)
	}
	defer func() { _ = locks.Unlock() }()
	if recovered := recoverLocked(ctx, identifier, t, recorded, refs, locks); recovered.Err != nil {
		return recovered
	}
	preview, built := buildPreview(context.WithoutCancel(ctx), r)
	if built != nil {
		return failedResult(ctx, identifier, t, built.failure(), nil)
	}
	return runLocked(ctx, preview, locks)
}

// Invalid and overlapping requests are argument errors.
func validateReferenceRequest(ctx context.Context, rendered *RenderedBundle, t *InstallationTarget, bundle *Bundle) (*TargetError, error) {
	_, built := references(ctx, rendered, nil, t, bundle, nil)
	switch {
	case built == nil:
		return nil, nil
	case built.target != nil:
		return built.target, nil
	}
	return nil, built.value
}
