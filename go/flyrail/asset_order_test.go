//go:build darwin || linux

package flyrail

import (
	"slices"
	"testing"
)

const assetClaimID = "asset"

type assetPlans struct {
	dependent, asset ResourceRef
	plans            []ResourcePlan
}

type assetCase struct {
	oldAsset, newAsset *revision
	stable             bool
	stillRequired      bool
}

func assetFixture(t *testing.T, c assetCase) assetPlans {
	t.Helper()
	dependent := ResourceRef{Destination: "/b/dependent", Kind: ResourceDocument}
	asset := ResourceRef{Destination: "/a/asset", Kind: ResourceDocument}
	required := requirement{path: asset.Destination, identifier: assetClaimID}
	oldDependent := ownedClaim{claimID: "dependent", requirements: []requirement{required}}
	newDependent := ownedClaim{claimID: "dependent"}
	if c.stable {
		oldDependent.stableRequirements = []requirement{required}
	}
	if c.stillRequired {
		newDependent.requirements = []requirement{required}
	}
	assetPlan := ResourcePlan{resource: asset}
	if c.oldAsset != nil {
		assetPlan.previous = &receipt{resource: asset, claims: []ownedClaim{{claimID: assetClaimID, installed: c.oldAsset}}}
	}
	if c.newAsset != nil {
		assetPlan.receipt = receipt{resource: asset, claims: []ownedClaim{{claimID: assetClaimID, installed: c.newAsset}}}
	}
	dependentPlan := ResourcePlan{
		resource: dependent,
		previous: &receipt{resource: dependent, claims: []ownedClaim{oldDependent}},
		receipt:  receipt{resource: dependent, claims: []ownedClaim{newDependent}},
	}
	return assetPlans{dependent: dependent, asset: asset, plans: []ResourcePlan{assetPlan, dependentPlan}}
}

func (f assetPlans) refs() map[string]ResourceRef {
	return map[string]ResourceRef{
		pathKey(f.dependent.Destination): f.dependent,
		pathKey(f.asset.Destination):     f.asset,
	}
}

func destinationsOf(plans []ResourcePlan) []string {
	destinations := make([]string, 0, len(plans))
	for _, plan := range plans {
		destinations = append(destinations, plan.resource.Destination)
	}
	return destinations
}

func revisionPointer(value revision) *revision { return &value }

func TestARetiredAssetIsOrderedAfterTheDependentThatDroppedItsReference(t *testing.T) {
	t.Parallel()
	installed := revisionPointer(fileRevision(t, "asset\n"))
	fixture := assetFixture(t, assetCase{oldAsset: installed})
	edges, failed := assetEdges(fixture.plans, fixture.plans[1], fixture.refs())
	if failed != nil {
		t.Fatal(failed)
	}
	if len(edges) != 1 || !resourceRefsEqual(edges[0].from, fixture.asset) || !resourceRefsEqual(edges[0].to, fixture.dependent) {
		t.Errorf("edges = %+v, want the asset to wait for the dependent", edges)
	}
	ordered, failed := orderPlans(fixture.plans, &RenderedBundle{}, nil)
	if failed != nil {
		t.Fatal(failed)
	}
	if got, want := destinationsOf(ordered), []string{fixture.dependent.Destination, fixture.asset.Destination}; !slices.Equal(got, want) {
		t.Errorf("order = %v, want %v", got, want)
	}
}

func TestAnAssetWithoutRetirementIsOrderedByDestination(t *testing.T) {
	t.Parallel()
	installed := revisionPointer(fileRevision(t, "asset\n"))
	fixture := assetFixture(t, assetCase{oldAsset: installed, newAsset: installed})
	ordered, failed := orderPlans(fixture.plans, &RenderedBundle{}, nil)
	if failed != nil {
		t.Fatal(failed)
	}
	if got, want := destinationsOf(ordered), []string{fixture.asset.Destination, fixture.dependent.Destination}; !slices.Equal(got, want) {
		t.Errorf("order = %v, want %v", got, want)
	}
}

func TestAnAssetWithALiveReferenceCannotBeRetired(t *testing.T) {
	t.Parallel()
	fixture := assetFixture(t, assetCase{oldAsset: revisionPointer(fileRevision(t, "asset\n")), stillRequired: true})
	_, failed := orderPlans(fixture.plans, &RenderedBundle{}, nil)
	if failed == nil || failed.Message != "cannot retire an asset with an active reference" {
		t.Errorf("error = %v, want the active-reference refusal", failed)
	}
}

func TestAReferencedAssetNeedsADistinctRevisionDestinationToChange(t *testing.T) {
	t.Parallel()
	changed := assetCase{oldAsset: revisionPointer(fileRevision(t, "one\n")), newAsset: revisionPointer(fileRevision(t, "two\n"))}
	fixture := assetFixture(t, changed)
	_, failed := orderPlans(fixture.plans, &RenderedBundle{}, nil)
	if failed == nil || failed.Message != "referenced asset changes require a distinct revision destination" {
		t.Errorf("error = %v, want the distinct-destination refusal", failed)
	}
	changed.stable = true
	fixture = assetFixture(t, changed)
	if _, failed := orderPlans(fixture.plans, &RenderedBundle{}, nil); failed != nil {
		t.Errorf("a stable requirement refused the change: %v", failed)
	}
}

func TestADependencyOutsideThePlannedResourcesIsRefused(t *testing.T) {
	t.Parallel()
	fixture := assetFixture(t, assetCase{oldAsset: revisionPointer(fileRevision(t, "asset\n"))})
	_, failed := assetEdges(fixture.plans[1:], fixture.plans[1], fixture.refs())
	if failed == nil || failed.Message != "planned dependency resource is missing" {
		t.Errorf("error = %v, want the missing-resource refusal", failed)
	}
}

func TestPlansThatRetireEachOthersAssetsAreACycle(t *testing.T) {
	t.Parallel()
	installed := revisionPointer(fileRevision(t, "asset\n"))
	first := ResourceRef{Destination: "/a/first", Kind: ResourceDocument}
	second := ResourceRef{Destination: "/a/second", Kind: ResourceDocument}
	plan := func(self, other ResourceRef) ResourcePlan {
		return ResourcePlan{
			resource: self,
			previous: &receipt{resource: self, claims: []ownedClaim{
				{claimID: assetClaimID, installed: installed},
				{claimID: "reference", requirements: []requirement{{path: other.Destination, identifier: assetClaimID}}},
			}},
			receipt: receipt{resource: self, claims: []ownedClaim{{claimID: "reference"}}},
		}
	}
	_, failed := orderPlans([]ResourcePlan{plan(first, second), plan(second, first)}, &RenderedBundle{}, nil)
	if failed == nil || failed.Message != "cyclic resource dependencies" {
		t.Errorf("error = %v, want the cycle refusal", failed)
	}
}

func TestRequirementSetsAddOnlyToKnownArtifactsWithoutDuplicates(t *testing.T) {
	t.Parallel()
	first, second := requirement{path: "/a", identifier: "x"}, requirement{path: "/b", identifier: "y"}
	sets := requirementSets{"guide": nil}
	if sets.add("other", first) {
		t.Error("add accepted an unknown artifact")
	}
	for _, item := range []requirement{first, second, first} {
		if !sets.add("guide", item) {
			t.Errorf("add refused %v for a known artifact", item)
		}
	}
	if got := sets["guide"]; !slices.Equal(got, []requirement{first, second}) {
		t.Errorf("requirements = %v, want each added once", got)
	}
	if _, present := sets["other"]; present {
		t.Error("add created an entry for an unknown artifact")
	}
}
