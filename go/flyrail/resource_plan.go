package flyrail

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"maps"
	"os"
	"slices"
	"strings"

	"github.com/nielsmadan/flyrail/go/flyrail/store"
	"github.com/nielsmadan/flyrail/go/internal/pathx"
	"github.com/nielsmadan/flyrail/go/internal/sysx"
)

// ResourcePlan is the immutable plan for one resource of a lifecycle preview.
type ResourcePlan struct {
	resource             ResourceRef
	before               revision
	after                revision
	previous             *receipt
	receipt              receipt
	ancestors            []ancestor
	transactionDirectory string
	recoveryPaths        []string
	requireExisting      bool
	err                  *TargetError
}

func newResourcePlan(p ResourcePlan) (ResourcePlan, error) {
	if !resourceRefsEqual(p.receipt.resource, p.resource) || p.previous != nil && !resourceRefsEqual(p.previous.resource, p.resource) {
		return ResourcePlan{}, newValueError("planned receipts disagree with resource")
	}
	if !pathx.IsAbsolute(p.transactionDirectory) || slices.ContainsFunc(p.recoveryPaths, func(path string) bool { return !pathx.IsAbsolute(path) }) {
		return ResourcePlan{}, newValueError("expected an absolute concrete path")
	}
	p.ancestors = slices.Clone(p.ancestors)
	p.recoveryPaths = slices.Clone(p.recoveryPaths)
	return p, nil
}

// Resource returns the planned resource.
func (p ResourcePlan) Resource() ResourceRef { return p.resource }

// TransactionDirectory returns the directory the plan's transaction stages in.
func (p ResourcePlan) TransactionDirectory() string { return p.transactionDirectory }

// RecoveryPaths returns the transaction directories that interrupted earlier transactions left for recovery.
func (p ResourcePlan) RecoveryPaths() []string { return slices.Clone(p.recoveryPaths) }

// RequireExisting reports whether the resource must already exist when the plan is applied.
func (p ResourcePlan) RequireExisting() bool { return p.requireExisting }

// Err returns a copy of the failure that prevents applying the plan, or nil.
func (p ResourcePlan) Err() *TargetError {
	return copyTargetError(p.err)
}

// Failure returns Err() as an error, or a nil error when there is no failure.
func (p ResourcePlan) Failure() error {
	return failureOf(p.Err())
}

func (p ResourcePlan) withAncestors(items []ancestor) ResourcePlan {
	p.ancestors = slices.Clone(items)
	p.recoveryPaths = slices.Clone(p.recoveryPaths)
	return p
}

func (p ResourcePlan) withError(failed *TargetError) ResourcePlan {
	p.after = p.before
	p.err = failed
	p.ancestors = slices.Clone(p.ancestors)
	p.recoveryPaths = slices.Clone(p.recoveryPaths)
	return p
}

// ClaimObservation is the observed state of one recorded claim.
type ClaimObservation struct {
	ClaimID      string
	BundleID     string
	ArtifactID   string
	Version      string
	BundleDigest string
	RenderDigest string
	Status       EditStatus
}

type resourceSummary struct {
	destination   string
	claims        []ClaimObservation
	contentDigest string
	recoveryPaths []string
}

func invalidAt(err error, path string) *TargetError {
	return newTargetError(CodeInvalidState, err.Error(), path, 0, err)
}

// Every I/O failure is io_error, as in Rust.
func planAuthorityError(err error) *TargetError {
	failed := errorDetail(err)
	if failed.Code == CodeUnsupported {
		failed.Code = CodeIOError
	}
	return failed
}

func resourceOf(ctx context.Context, artifact RenderedArtifact) (ResourceRef, *TargetError) {
	authority, err := NewResourceAuthority(ctx, artifact.destination)
	if err != nil {
		return ResourceRef{}, planAuthorityError(err)
	}
	kind := ResourceDocument
	if artifact.subtree != "" {
		kind = ResourceSkillContainer
	} else if _, ok := artifact.content.(TreeContent); ok {
		kind = ResourceTree
	}
	reference, err := newResourceRef(authority.destination, kind, stateSchemaVersion)
	if err != nil {
		return ResourceRef{}, invalidAt(err, artifact.destination)
	}
	return reference, nil
}

func sameOptionalCount(a, b count) bool {
	left, okLeft := countU32(a)
	right, okRight := countU32(b)
	return okLeft == okRight && left == right
}

func intact(actual, expected revision) bool {
	return slices.EqualFunc(actual.nodes, expected.nodes, func(a, b node) bool {
		return a.path == b.path && a.hasData == b.hasData && string(a.data) == string(b.data) && sameOptionalCount(a.mode, b.mode)
	})
}

func rebaseNode(item node, path string) (node, error) {
	return newNode(path, item.data, item.hasData, item.fileIdentity, item.security)
}

func subtreeOf(claim Claim) (string, bool) {
	selector, ok := claim.selector.(SubtreeSelector)
	return string(selector), ok
}

func selectedRevision(current revision, selector string, hasSelector bool) (revision, error) {
	if !hasSelector {
		return current, nil
	}
	prefix := selector + "/"
	var nodes []node
	for _, item := range current.nodes {
		if item.path != selector && !strings.HasPrefix(item.path, prefix) {
			continue
		}
		path := strings.TrimPrefix(item.path, prefix)
		if item.path == selector {
			path = ""
		}
		rebased, err := rebaseNode(item, path)
		if err != nil {
			return revision{}, err
		}
		nodes = append(nodes, rebased)
	}
	return newRevision(nodes)
}

func processOwner() (count, count) {
	return count{small: int64(os.Geteuid())}, count{small: int64(os.Getegid())}
}

func newPlannedNode(path string, data []byte, hasData bool, mode int64) (node, error) {
	uid, gid := processOwner()
	return newNode(path, data, hasData, fileIdentity{mode: count{small: mode}, uid: uid, gid: gid}, nil)
}

func replaceSelection(current revision, claim Claim, content revision) (revision, error) {
	selector, ok := subtreeOf(claim)
	if !ok {
		return content, nil
	}
	prefix := selector + "/"
	var nodes []node
	for _, item := range current.nodes {
		if item.path != selector && !strings.HasPrefix(item.path, prefix) {
			nodes = append(nodes, item)
		}
	}
	if len(content.nodes) > 0 && len(nodes) == 0 {
		root, err := newPlannedNode("", nil, false, 0o755)
		if err != nil {
			return revision{}, err
		}
		nodes = append(nodes, root)
	}
	for _, item := range content.nodes {
		path := selector
		if item.path != "" {
			path = prefix + item.path
		}
		rebased, err := rebaseNode(item, path)
		if err != nil {
			return revision{}, err
		}
		nodes = append(nodes, rebased)
	}
	if len(content.nodes) > 0 {
		parts := strings.Split(selector, "/")
		for index := 1; index < len(parts); index++ {
			parent := strings.Join(parts[:index], "/")
			if slices.ContainsFunc(nodes, func(item node) bool { return item.path == parent }) {
				continue
			}
			directory, err := newPlannedNode(parent, nil, false, 0o755)
			if err != nil {
				return revision{}, err
			}
			nodes = append(nodes, directory)
		}
	}
	slices.SortFunc(nodes, func(a, b node) int { return strings.Compare(a.path, b.path) })
	return newRevision(nodes)
}

func desiredRevision(content Content) (revision, error) {
	switch value := content.(type) {
	case FileContent:
		mode := map[FileMode]int64{ModePrivate: 0o600, ModeReadable: 0o644, ModeExecutable: 0o755}[value.mode]
		root, err := newPlannedNode("", value.data, true, mode)
		if err != nil {
			return revision{}, err
		}
		return newRevision([]node{root})
	case TreeContent:
		root, err := newPlannedNode("", nil, false, 0o755)
		if err != nil {
			return revision{}, err
		}
		nodes := []node{root}
		for _, entry := range value.entries {
			mode := int64(0o644)
			if entry.Dir || entry.Executable {
				mode = 0o755
			}
			item, err := newPlannedNode(entry.Path, entry.Data, !entry.Dir, mode)
			if err != nil {
				return revision{}, err
			}
			nodes = append(nodes, item)
		}
		return newRevision(nodes)
	}
	return revision{}, newValueError("whole resource requires file or tree content")
}

type claimStatus struct {
	claim  Claim
	status EditStatus
}

func withStatus(statuses []claimStatus, claim Claim, status EditStatus) []claimStatus {
	result := slices.Clone(statuses)
	if index := slices.IndexFunc(result, func(item claimStatus) bool { return item.claim.Equal(claim) }); index >= 0 {
		result[index].status = status
		return result
	}
	return append(result, claimStatus{claim: claim, status: status})
}

func revisionDocument(current revision) document {
	data, exists := current.data()
	return document{data: data, exists: exists}
}

func requirementsMatch(current document, requirements []DocumentSchema) (bool, error) {
	all := true
	for _, requirement := range requirements {
		matches, err := matchesSchema(current.data, current.exists, requirement)
		if err != nil {
			return false, err
		}
		all = all && matches
	}
	return all, nil
}

func claimObservations(recorded receipt, current revision) ([]ClaimObservation, error) {
	var selections []OwnedSelection
	for _, owned := range recorded.claims {
		if owned.selection != nil {
			selections = append(selections, *owned.selection)
		}
	}
	observed, err := observeSelections(revisionDocument(current), selections)
	if err != nil {
		return nil, err
	}
	statuses := make([]claimStatus, 0, len(observed))
	for _, item := range observed {
		statuses = withStatus(statuses, item.Claim, item.Status)
	}
	for _, owned := range recorded.claims {
		if owned.installed == nil {
			continue
		}
		selector, hasSelector := subtreeOf(owned.claim)
		actual, err := selectedRevision(current, selector, hasSelector)
		if err != nil {
			return nil, err
		}
		status := EditModified
		if intact(actual, *owned.installed) {
			status = EditCurrent
		}
		statuses = withStatus(statuses, owned.claim, status)
	}
	for _, owned := range recorded.claims {
		matches, err := requirementsMatch(revisionDocument(current), owned.schemaRequirements)
		if err != nil {
			return nil, err
		}
		if !matches {
			statuses = withStatus(statuses, owned.claim, EditModified)
		}
	}
	result := make([]ClaimObservation, 0, len(recorded.claims))
	for _, owned := range recorded.claims {
		index := slices.IndexFunc(statuses, func(item claimStatus) bool { return item.claim.Equal(owned.claim) })
		if index < 0 {
			return nil, newValueError("claim observation is missing")
		}
		result = append(result, ClaimObservation{
			ClaimID:      owned.claimID,
			BundleID:     owned.bundleID,
			ArtifactID:   owned.artifactID,
			Version:      owned.version,
			BundleDigest: owned.bundleDigest,
			RenderDigest: owned.renderDigest,
			Status:       statuses[index].status,
		})
	}
	return result, nil
}

func journalKey(reference ResourceRef) string { return resourceKey(reference.Destination, "journal") }

func receiptKey(reference ResourceRef) string { return resourceKey(reference.Destination, "receipt") }

func preparationKey(reference ResourceRef) string {
	return resourceKey(reference.Destination, "preparation")
}

func knownPreparation(st store.Store, prepared preparation) bool {
	return recognizedTransactionDirectory(st, prepared.resource.Destination, prepared.transactionID, prepared.directory)
}

func unrecognizedDirectory(st store.Store) *TargetError {
	return stateFailure(st, CodeRecoveryNeeded, "transaction record names an unrecognized directory")
}

func absentPath(path string) bool {
	_, err := sysx.LstatMetadata(path)
	return notFound(err)
}

func matchingPreparation(st store.Store, prepared *preparation, recorded journal) (preparation, *TargetError) {
	if prepared == nil || !resourceRefsEqual(prepared.resource, recorded.resource) || prepared.transactionID != recorded.receipt.transactionID {
		return preparation{}, stateFailure(st, CodeInvalidState, "transaction record has no matching preparation")
	}
	return *prepared, nil
}

func transactionRecords(ctx context.Context, st store.Store, reference ResourceRef) (*preparation, *journal, *TargetError) {
	prepared, hasPreparation, failed := readRecord(ctx, st, preparationKey(reference), "Preparation", preparationFromState)
	if failed != nil {
		return nil, nil, failed
	}
	var selected *preparation
	if hasPreparation {
		selected = &prepared
		if !knownPreparation(st, prepared) && !absentPath(prepared.directory) {
			return nil, nil, unrecognizedDirectory(st)
		}
	}
	recorded, hasJournal, failed := readRecord(ctx, st, journalKey(reference), "Journal", journalFromState)
	if failed != nil {
		return nil, nil, failed
	}
	if !hasJournal {
		return selected, nil, nil
	}
	if selected != nil && !knownPreparation(st, *selected) {
		return nil, nil, unrecognizedDirectory(st)
	}
	if _, failed := matchingPreparation(st, selected, recorded); failed != nil {
		return nil, nil, failed
	}
	return selected, &recorded, nil
}

func pendingRecovery(ctx context.Context, st store.Store, reference ResourceRef) ([]string, *TargetError) {
	prepared, _, failed := transactionRecords(ctx, st, reference)
	if failed != nil {
		return nil, failed
	}
	if prepared == nil {
		return nil, nil
	}
	return []string{prepared.directory}, nil
}

func storedReceipt(ctx context.Context, st store.Store, reference ResourceRef) (*receipt, *TargetError) {
	recorded, ok, failed := readRecord(ctx, st, receiptKey(reference), "Receipt", receiptFromState)
	if failed != nil {
		return nil, failed
	}
	if !ok {
		return nil, nil
	}
	if !resourceRefsEqual(recorded.resource, reference) {
		return nil, failure(CodeInvalidState, "receipt resource disagrees", reference.Destination)
	}
	return &recorded, nil
}

func summarizeResource(ctx context.Context, st store.Store, reference ResourceRef) (resourceSummary, *TargetError) {
	recoveryPaths, failed := pendingRecovery(ctx, st, reference)
	if failed != nil {
		return resourceSummary{}, failed
	}
	recorded, failed := storedReceipt(ctx, st, reference)
	if failed != nil {
		return resourceSummary{}, failed
	}
	current, failed := observe(reference.Destination)
	if failed != nil {
		return resourceSummary{}, failed
	}
	var claims []ClaimObservation
	if recorded != nil {
		var err error
		if claims, err = claimObservations(*recorded, current); err != nil {
			return resourceSummary{}, invalidAt(err, reference.Destination)
		}
	}
	return resourceSummary{
		destination:   reference.Destination,
		claims:        claims,
		contentDigest: current.contentDigest(),
		recoveryPaths: recoveryPaths,
	}, nil
}

type plannedClaim struct {
	artifact           RenderedArtifact
	bundle             *Bundle
	rendered           *RenderedBundle
	reference          ResourceRef
	requirements       []requirement
	stableRequirements []requirement
	selection          *OwnedSelection
	installed          *revision
	disposition        Disposition
	baseline           *revision
}

func ownedClaimFor(p plannedClaim) (ownedClaim, error) {
	claim := artifactClaim(p.artifact)
	identity, err := claim.identity(p.reference.Destination)
	if err != nil {
		return ownedClaim{}, err
	}
	return newOwnedClaim(ownedClaim{
		claim:              claim,
		claimID:            identity,
		bundleID:           p.bundle.ID(),
		artifactID:         p.artifact.id,
		version:            p.bundle.Version(),
		bundleDigest:       p.bundle.ContentDigest(),
		renderDigest:       p.rendered.ContentDigest(),
		selection:          p.selection,
		installed:          p.installed,
		disposition:        p.disposition,
		baseline:           p.baseline,
		requirements:       p.requirements,
		stableRequirements: p.stableRequirements,
		schemaRequirements: p.artifact.SchemaRequirements(),
	})
}

func artifactRequirements(artifact RenderedArtifact, requirements, stableRequirements map[string][]requirement) ([]requirement, []requirement, error) {
	required, ok := requirements[artifact.id]
	stable, okStable := stableRequirements[artifact.id]
	if !ok || !okStable {
		return nil, nil, newValueError("planned artifact requirements are missing")
	}
	return slices.Clone(required), slices.Clone(stable), nil
}

func ownsBelow(path, selector string) bool {
	return path == "" || strings.HasPrefix(selector, path+"/")
}

func containerProvenance(before, after revision, previous *receipt, claims []ownedClaim) (revision, []node, error) {
	existing := make(map[string]node, len(before.nodes))
	for _, item := range before.nodes {
		existing[item.path] = item
	}
	created := map[string]node{}
	if previous != nil {
		for _, item := range previous.createdDirectories {
			if current, ok := existing[item.path]; ok && current.equal(item) {
				created[item.path] = item
			}
		}
	}
	var selectors []string
	for _, owned := range claims {
		if selector, ok := subtreeOf(owned.claim); ok {
			selectors = append(selectors, selector)
		}
	}
	nodes := make(map[string]node, len(after.nodes))
	for _, item := range after.nodes {
		nodes[item.path] = item
	}
	for _, item := range after.nodes {
		if _, seen := existing[item.path]; item.hasData || seen {
			continue
		}
		if slices.ContainsFunc(selectors, func(selector string) bool { return ownsBelow(item.path, selector) }) {
			created[item.path] = item
		}
	}
	paths := slices.Sorted(maps.Keys(created))
	slices.Reverse(paths)
	var retained []node
	for _, path := range paths {
		if slices.ContainsFunc(selectors, func(selector string) bool { return ownsBelow(path, selector) }) {
			retained = append(retained, created[path])
			continue
		}
		inUse := false
		for other := range nodes {
			if other != path && (path == "" || strings.HasPrefix(other, path+"/")) {
				inUse = true
				break
			}
		}
		if !inUse {
			delete(nodes, path)
		}
	}
	byPath := func(a, b node) int { return strings.Compare(a.path, b.path) }
	finalNodes := slices.SortedFunc(maps.Values(nodes), byPath)
	slices.SortFunc(retained, byPath)
	final, err := newRevision(finalNodes)
	if err != nil {
		return revision{}, nil, err
	}
	return final, retained, nil
}

func newTransactionID() string {
	var random [16]byte
	_, _ = rand.Read(random[:])
	return hex.EncodeToString(random[:])
}

type desiredClaim struct {
	claim    Claim
	artifact RenderedArtifact
}

type planInputs struct {
	reference          ResourceRef
	identifier         string
	bundle             *Bundle
	rendered           *RenderedBundle
	artifacts          []RenderedArtifact
	acquisition        Acquisition
	replaceModified    bool
	requirements       map[string][]requirement
	stableRequirements map[string][]requirement
}

func (s OwnedSelection) withInstalled(installed SelectionSnapshot) (OwnedSelection, error) {
	return NewOwnedSelection(OwnedSelectionParams{
		Claim:       s.claim,
		Format:      s.format,
		Installed:   installed,
		Disposition: s.disposition,
		Baseline:    s.baseline,
		Separator:   s.separator,
	})
}

func plannedDocument(before revision, data []byte, exists bool) (revision, error) {
	if !exists {
		return newRevision(nil)
	}
	if len(before.nodes) > 0 {
		first := before.nodes[0]
		item, err := newNode(first.path, data, true, first.fileIdentity, first.security)
		if err != nil {
			return revision{}, err
		}
		rebased, err := rebaseNode(item, "")
		if err != nil {
			return revision{}, err
		}
		return newRevision([]node{rebased})
	}
	root, err := newPlannedNode("", data, true, 0o600)
	if err != nil {
		return revision{}, err
	}
	return newRevision([]node{root})
}

type sharedPlan struct {
	in         planInputs
	before     revision
	after      revision
	claims     []ownedClaim
	own        []ownedClaim
	desired    []desiredClaim
	provenance DocumentProvenance
	result     []ownedClaim
	err        *TargetError
}

func (s *sharedPlan) ownFor(claim Claim) *ownedClaim {
	index := slices.IndexFunc(s.own, func(owned ownedClaim) bool { return owned.claim.Equal(claim) })
	if index < 0 {
		return nil
	}
	return &s.own[index]
}

func (s *sharedPlan) desiredFor(claim Claim) *desiredClaim {
	index := slices.IndexFunc(s.desired, func(item desiredClaim) bool { return item.claim.Equal(claim) })
	if index < 0 {
		return nil
	}
	return &s.desired[index]
}

func (s *sharedPlan) invalid(err error) *TargetError {
	return invalidAt(err, s.in.reference.Destination)
}

func (s *sharedPlan) editDocument(requests []DocumentRequest, selections []OwnedSelection, active []DocumentSchema) (DocumentEdit, *TargetError) {
	data, exists := s.before.data()
	edit, err := editSchemaDocument(data, exists, requests, selections, s.provenance, active, len(s.in.artifacts) > 0)
	if err != nil {
		return DocumentEdit{}, s.invalid(err)
	}
	return edit, nil
}

func (s *sharedPlan) planSharedDocument() *TargetError {
	destination := s.in.reference.Destination
	if _, isFile := s.before.data(); len(s.before.nodes) > 0 && !isFile {
		s.err = failure(CodeUnsafePath, "shared document is not a file", destination)
	}
	var selections []OwnedSelection
	for _, owned := range s.claims {
		if owned.selection != nil {
			selections = append(selections, *owned.selection)
		}
	}
	var requests []DocumentRequest
	for _, owned := range s.own {
		if s.desiredFor(owned.claim) != nil {
			continue
		}
		request, err := NewRetireRequest(owned.claim)
		if err != nil {
			return s.invalid(err)
		}
		requests = append(requests, request)
	}
	for _, artifact := range s.in.artifacts {
		switch artifact.content.(type) {
		case SectionContent, StructuredContent:
			request, err := NewSetRequest(artifact.content, s.in.acquisition)
			if err != nil {
				return s.invalid(err)
			}
			requests = append(requests, request)
		default:
			s.err = failure(CodeConflict, "whole and partial claims cannot share a document", destination)
		}
	}
	if s.err != nil {
		return nil
	}
	var active []DocumentSchema
	for _, owned := range s.result {
		active = append(active, owned.schemaRequirements...)
	}
	for _, artifact := range s.in.artifacts {
		active = append(active, artifact.schemaRequirements...)
	}
	edit, failed := s.editDocument(requests, selections, active)
	if failed != nil {
		return failed
	}
	if s.in.replaceModified && !edit.Applicable() {
		for _, observed := range edit.Observations {
			if observed.Status != EditModified || observed.Current == nil {
				continue
			}
			index := slices.IndexFunc(selections, func(item OwnedSelection) bool { return item.claim.Equal(observed.Claim) })
			if index < 0 {
				continue
			}
			replaced, err := selections[index].withInstalled(*observed.Current)
			if err != nil {
				return s.invalid(err)
			}
			selections[index] = replaced
		}
		if edit, failed = s.editDocument(requests, selections, active); failed != nil {
			return failed
		}
	}
	if !edit.Applicable() {
		index := slices.IndexFunc(edit.Observations, func(item EditObservation) bool {
			return item.Status == EditModified || item.Status == EditConflict
		})
		if index >= 0 {
			issue := edit.Observations[index]
			code := CodeConflict
			if issue.Status == EditModified {
				code = CodeModified
			}
			message := issue.Message
			if message == "" {
				message = "owned selection cannot be changed"
			}
			s.err = failure(code, message, destination)
		}
		return nil
	}
	after, err := plannedDocument(s.before, edit.After, edit.AfterExists)
	if err != nil {
		return s.invalid(err)
	}
	s.after = after
	s.provenance = edit.Provenance
	for _, wanted := range s.desired {
		if s.in.bundle == nil {
			return failure(CodeInvalidState, "desired content requires a bundle", destination)
		}
		index := slices.IndexFunc(edit.Ownership, func(owned OwnedSelection) bool { return owned.claim.Equal(wanted.claim) })
		if index < 0 {
			return failure(CodeInvalidState, "planned selection ownership is missing", destination)
		}
		selection := edit.Ownership[index]
		required, stable, err := artifactRequirements(wanted.artifact, s.in.requirements, s.in.stableRequirements)
		if err != nil {
			return s.invalid(err)
		}
		owned, err := ownedClaimFor(plannedClaim{
			artifact: wanted.artifact, bundle: s.in.bundle, rendered: s.in.rendered, reference: s.in.reference,
			requirements: required, stableRequirements: stable, selection: &selection, disposition: selection.disposition,
		})
		if err != nil {
			return s.invalid(err)
		}
		s.result = append(s.result, owned)
	}
	return nil
}

func (s *sharedPlan) planWholeClaims() *TargetError {
	destination := s.in.reference.Destination
	var all []Claim
	for _, owned := range s.own {
		all = append(all, owned.claim)
	}
	for _, wanted := range s.desired {
		all = append(all, wanted.claim)
	}
	identity := func(claim Claim) string {
		value, _ := claim.identity(destination)
		return value
	}
	slices.SortStableFunc(all, func(a, b Claim) int { return strings.Compare(identity(a), identity(b)) })
	all = slices.CompactFunc(all, Claim.Equal)
	for _, claim := range all {
		old := s.ownFor(claim)
		wanted := s.desiredFor(claim)
		selector, hasSelector := subtreeOf(claim)
		actual, err := selectedRevision(s.before, selector, hasSelector)
		if err != nil {
			return s.invalid(err)
		}
		if old != nil && old.installed != nil && !intact(actual, *old.installed) && !s.in.replaceModified {
			s.err = failure(CodeModified, "owned resource has local modifications", destination)
			break
		}
		var replacement revision
		if wanted != nil {
			if s.in.bundle == nil {
				return failure(CodeInvalidState, "desired content requires a bundle", destination)
			}
			required, stable, err := artifactRequirements(wanted.artifact, s.in.requirements, s.in.stableRequirements)
			if err != nil {
				return s.invalid(err)
			}
			if replacement, err = desiredRevision(wanted.artifact.content); err != nil {
				return s.invalid(err)
			}
			disposition := DispositionCreated
			var baseline *revision
			if old != nil {
				disposition = old.disposition
				baseline = old.baseline
			}
			if old == nil && len(actual.nodes) > 0 {
				switch {
				case s.in.acquisition == AcquireConflict:
					s.err = failure(CodeConflict, "unowned content already exists", destination)
				case s.in.acquisition == AcquireAdopt && !intact(actual, replacement):
					s.err = failure(CodeConflict, "adoption requires matching content", destination)
				case s.in.acquisition == AcquireAdopt:
					disposition = DispositionAdopted
				default:
					disposition = DispositionTakenOver
					captured := actual
					baseline = &captured
				}
				if s.err != nil {
					break
				}
			}
			installed := replacement
			owned, err := ownedClaimFor(plannedClaim{
				artifact: wanted.artifact, bundle: s.in.bundle, rendered: s.in.rendered, reference: s.in.reference,
				requirements: required, stableRequirements: stable, installed: &installed, disposition: disposition, baseline: baseline,
			})
			if err != nil {
				return s.invalid(err)
			}
			s.result = append(s.result, owned)
		} else if old != nil && old.baseline != nil {
			replacement = *old.baseline
		}
		if !intact(actual, replacement) {
			if s.after, err = replaceSelection(s.after, claim, replacement); err != nil {
				return s.invalid(err)
			}
		}
	}
	return nil
}

func planResource(ctx context.Context, st store.Store, in planInputs) (ResourcePlan, *TargetError) {
	reference := in.reference
	destination := reference.Destination
	if ctx != nil && ctx.Err() != nil {
		return ResourcePlan{}, newTargetError(CodeIOError, ctx.Err().Error(), destination, 0, ctx.Err())
	}
	recoveryPaths, failed := pendingRecovery(ctx, st, reference)
	if failed != nil {
		return ResourcePlan{}, failed
	}
	previous, failed := storedReceipt(ctx, st, reference)
	if failed != nil {
		return ResourcePlan{}, failed
	}
	before, failed := observe(destination)
	if failed != nil {
		return ResourcePlan{}, failed
	}
	s := sharedPlan{in: in, before: before, after: before}
	if previous != nil {
		s.claims = previous.claims
		s.provenance = previous.provenance
	}
	for _, owned := range s.claims {
		if owned.bundleID == in.identifier {
			s.own = append(s.own, owned)
		} else {
			s.result = append(s.result, owned)
		}
	}
	for _, artifact := range in.artifacts {
		claim := artifactClaim(artifact)
		if slices.ContainsFunc(s.desired, func(item desiredClaim) bool { return item.claim.Equal(claim) }) {
			return ResourcePlan{}, failure(CodeInvalidState, "rendered artifacts duplicate a physical claim", destination)
		}
		s.desired = append(s.desired, desiredClaim{claim: claim, artifact: artifact})
	}
	for _, wanted := range s.desired {
		if slices.ContainsFunc(s.claims, func(existing ownedClaim) bool {
			return wanted.claim.Overlaps(existing.claim) && (existing.bundleID != in.identifier || !wanted.claim.Equal(existing.claim))
		}) {
			s.err = failure(CodeConflict, "claim overlaps recorded ownership", destination)
		}
	}
	required := slices.ContainsFunc(in.artifacts, RenderedArtifact.RequireExisting)
	if required && len(before.nodes) == 0 {
		s.err = failure(CodeConflict, "resource must already exist", destination)
	}
	if _, isFile := before.data(); len(before.nodes) > 0 && isFile != (reference.Kind == ResourceDocument) {
		s.err = failure(CodeUnsafePath, "resource kind disagrees with existing destination", destination)
	}
	shared := func(claim Claim) bool { return claim.kind == OwnershipSection || claim.kind == OwnershipStructured }
	sharedDocument := reference.Kind == ResourceDocument &&
		(slices.ContainsFunc(s.own, func(owned ownedClaim) bool { return shared(owned.claim) }) ||
			slices.ContainsFunc(s.desired, func(item desiredClaim) bool { return shared(item.claim) }))
	switch {
	case sharedDocument:
		if failed := s.planSharedDocument(); failed != nil {
			return ResourcePlan{}, failed
		}
	case s.err == nil:
		if failed := s.planWholeClaims(); failed != nil {
			return ResourcePlan{}, failed
		}
	}
	var createdDirectories []node
	if previous != nil {
		createdDirectories = previous.createdDirectories
	}
	if reference.Kind == ResourceSkillContainer && s.err == nil {
		var err error
		if s.after, createdDirectories, err = containerProvenance(before, s.after, previous, s.result); err != nil {
			return ResourcePlan{}, s.invalid(err)
		}
	}
	slices.SortStableFunc(s.result, func(a, b ownedClaim) int { return strings.Compare(a.claimID, b.claimID) })
	var planned receipt
	var err error
	switch {
	case s.err == nil:
		planned, err = newReceipt(reference, newTransactionID(), s.result, s.provenance, createdDirectories, stateSchemaVersion)
	case previous != nil:
		planned = *previous
	default:
		planned, err = newReceipt(reference, newTransactionID(), nil, DocumentProvenance{}, nil, stateSchemaVersion)
	}
	if err != nil {
		return ResourcePlan{}, s.invalid(err)
	}
	after := s.after
	if s.err != nil {
		after = before
	}
	if after.equal(before) && previous != nil && planned.equivalentExceptTransaction(*previous) {
		planned = *previous
	}
	directory, failed := chooseTransactionDirectory(st, destination, planned.transactionID)
	if failed != nil {
		return ResourcePlan{}, failed
	}
	captured, failed := resourceAncestors(destination, pathx.Join(directory, "staging"))
	if failed != nil {
		return ResourcePlan{}, failed
	}
	plan, err := newResourcePlan(ResourcePlan{
		resource:             reference,
		before:               before,
		after:                after,
		previous:             previous,
		receipt:              planned,
		ancestors:            captured,
		transactionDirectory: directory,
		recoveryPaths:        recoveryPaths,
		requireExisting:      required,
		err:                  s.err,
	})
	if err != nil {
		return ResourcePlan{}, s.invalid(err)
	}
	return plan, nil
}

// The order resource locks are taken in.
func destinationOrder(resources []ResourceRef) []ResourceRef {
	ordered := slices.Clone(resources)
	slices.SortStableFunc(ordered, func(a, b ResourceRef) int {
		return strings.Compare(posixPath(a.Destination), posixPath(b.Destination))
	})
	return ordered
}

func isSourceFreeRemoval(recorded receipt, bundleID string) bool {
	return !slices.ContainsFunc(recorded.claims, func(owned ownedClaim) bool { return owned.bundleID != bundleID })
}

func expectsReceipt(recorded *index, reference ResourceRef) bool {
	if recorded == nil {
		return false
	}
	for _, generation := range []*generation{recorded.current, recorded.previous} {
		if generation != nil && slices.ContainsFunc(generation.membership, func(member membership) bool {
			return resourceRefsEqual(member.resource, reference)
		}) {
			return true
		}
	}
	return false
}

func pendingIndex(identifier, contextDigest string, previous *index, pending *generation, resources []ResourceRef) (index, error) {
	var current *generation
	if previous != nil {
		current = previous.current
	}
	return newIndex(identifier, contextDigest, current, pending, current, resources, stateSchemaVersion)
}

func completedIndex(identifier, contextDigest string, current *generation) (index, error) {
	return newIndex(identifier, contextDigest, current, nil, nil, nil, stateSchemaVersion)
}
