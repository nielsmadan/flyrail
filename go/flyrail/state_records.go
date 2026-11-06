package flyrail

import (
	"slices"
	"strconv"
	"strings"

	"github.com/nielsmadan/flyrail/go/internal/jsonx"
	"github.com/nielsmadan/flyrail/go/internal/pathx"
	"github.com/nielsmadan/flyrail/go/internal/unicodex"
)

// ResourceKind is the shape of a managed resource.
type ResourceKind string

// The resource kinds, spelled as on the wire.
const (
	ResourceDocument       ResourceKind = "document"
	ResourceTree           ResourceKind = "tree"
	ResourceSkillContainer ResourceKind = "skill-container"
)

// ParseResourceKind returns the ResourceKind spelled s.
func ParseResourceKind(s string) (ResourceKind, error) {
	return parseEnum(s, "ResourceKind", ResourceDocument, ResourceTree, ResourceSkillContainer)
}

// ResourceRef identifies one managed resource by its canonical destination and kind.
type ResourceRef struct {
	Destination string
	Kind        ResourceKind
}

// SchemaVersion returns the resource schema version Flyrail records.
func (ResourceRef) SchemaVersion() int { return stateSchemaVersion }

const stateSchemaVersion = 2

func newResourceRef(destination string, kind ResourceKind, schemaVersion int64) (ResourceRef, error) {
	if schemaVersion != stateSchemaVersion {
		return ResourceRef{}, newValueError("authority schema must be 2")
	}
	if err := checkAbsolute(destination); err != nil {
		return ResourceRef{}, err
	}
	if err := checkEnum(kind, "ResourceKind", ResourceDocument, ResourceTree, ResourceSkillContainer); err != nil {
		return ResourceRef{}, err
	}
	return ResourceRef{Destination: destination, Kind: kind}, nil
}

func resourceRefsEqual(a, b ResourceRef) bool {
	return a.Kind == b.Kind && pathsEqual(a.Destination, b.Destination)
}

func pathKey(path string) string {
	var key strings.Builder
	for _, component := range pathx.Components(path) {
		key.WriteString(strconv.Itoa(len(component)))
		key.WriteByte(':')
		key.WriteString(component)
	}
	return key.String()
}

func checkHexadecimal(value string, length int) error {
	if len(value) != length || strings.IndexFunc(value, func(r rune) bool {
		return (r < '0' || r > '9') && (r < 'a' || r > 'f')
	}) >= 0 {
		return newValueError("invalid hexadecimal identity")
	}
	return nil
}

func hasParentDir(path string) bool {
	return slices.Contains(pathx.Components(path), "..")
}

func checkAbsolute(path string) error {
	if !pathx.IsAbsolute(path) || hasParentDir(path) {
		return newValueError("expected an absolute concrete path")
	}
	if !hasParent(path) {
		return newValueError("filesystem roots are not resources")
	}
	return nil
}

func stateDigest(record jsonx.Value) string {
	return sha256Hex(encodeState(record))
}

// big holds the decimal text of values outside int64.
type count struct {
	small int64
	big   string
}

func (c count) negative() bool {
	if c.big != "" {
		return strings.HasPrefix(c.big, "-")
	}
	return c.small < 0
}

func (c count) exceeds(limit int64) bool {
	return c.big != "" || c.small > limit
}

func (c count) json() jsonx.Value {
	if c.big != "" {
		return jsonx.BigInt(c.big)
	}
	return jsonx.Int(c.small)
}

func (c count) uint64() (uint64, bool) {
	if c.big != "" {
		value, err := strconv.ParseUint(c.big, 10, 64)
		return value, err == nil
	}
	if c.small < 0 {
		return 0, false
	}
	return uint64(c.small), true
}

func anyNegative(counts ...count) bool {
	return slices.ContainsFunc(counts, count.negative)
}

type fileIdentity struct {
	device count
	inode  count
	mode   count
	uid    count
	gid    count
}

func (f fileIdentity) valid(maxMode int64) bool {
	return !anyNegative(f.device, f.inode, f.mode, f.uid, f.gid) && !f.mode.exceeds(maxMode)
}

type ancestor struct {
	fileIdentity
	path     string
	security []byte
}

func newAncestor(path string, identity fileIdentity, security []byte) (ancestor, error) {
	if !pathx.IsAbsolute(path) {
		return ancestor{}, newValueError("ancestor path must be absolute")
	}
	if !identity.valid(0o7777) {
		return ancestor{}, newValueError("invalid ancestor metadata")
	}
	return ancestor{fileIdentity: identity, path: path, security: cloneBytes(security)}, nil
}

type node struct {
	fileIdentity
	path     string
	data     []byte
	hasData  bool
	security []byte
}

func newNode(path string, data []byte, hasData bool, identity fileIdentity, security []byte) (node, error) {
	if path != "" {
		if err := validateRelativePath(path, "revision path"); err != nil {
			return node{}, err
		}
	}
	if !identity.valid(0o777) {
		return node{}, newValueError("invalid revision metadata")
	}
	result := node{fileIdentity: identity, path: path, hasData: hasData, security: cloneBytes(security)}
	if hasData {
		result.data = cloneBytes(data)
	}
	return result, nil
}

func (n node) equal(o node) bool {
	return n.path == o.path && n.hasData == o.hasData && string(n.data) == string(o.data) &&
		n.fileIdentity == o.fileIdentity && string(n.security) == string(o.security)
}

type revision struct {
	nodes []node
}

func newRevision(nodes []node) (revision, error) {
	byPath := make(map[string]node, len(nodes))
	for _, item := range nodes {
		if _, seen := byPath[item.path]; seen {
			return revision{}, newValueError("revision must have exactly one root")
		}
		byPath[item.path] = item
	}
	if len(nodes) > 0 && nodes[0].path != "" {
		return revision{}, newValueError("revision must have exactly one root")
	}
	for _, item := range nodes[min(1, len(nodes)):] {
		parentPath := ""
		if cut := strings.LastIndexByte(item.path, '/'); cut >= 0 {
			parentPath = item.path[:cut]
		}
		if parent, ok := byPath[parentPath]; !ok || parent.hasData {
			return revision{}, newValueError("revision parents must be directories")
		}
	}
	if !slices.IsSortedFunc(nodes, func(a, b node) int { return strings.Compare(a.path, b.path) }) {
		return revision{}, newValueError("revision entries must be ordered")
	}
	return revision{nodes: slices.Clone(nodes)}, nil
}

func (r revision) data() ([]byte, bool) {
	if len(r.nodes) == 0 || !r.nodes[0].hasData {
		return nil, false
	}
	return r.nodes[0].data, true
}

func (r revision) contentDigest() string {
	entries := make([]jsonx.Value, 0, len(r.nodes))
	for _, item := range r.nodes {
		data := jsonx.Null()
		if item.hasData {
			data = bytesState(item.data)
		}
		entries = append(entries, jsonx.Array(jsonx.String(item.path), data, item.mode.json()))
	}
	return stateDigest(jsonx.Array(entries...))
}

func (r revision) equal(o revision) bool {
	return slices.EqualFunc(r.nodes, o.nodes, node.equal)
}

type preparation struct {
	resource      ResourceRef
	transactionID string
	directory     string
}

func newPreparation(resource ResourceRef, transactionID, directory string, schemaVersion int64) (preparation, error) {
	if err := checkHexadecimal(transactionID, 32); err != nil {
		return preparation{}, err
	}
	if err := checkAbsolute(directory); err != nil {
		return preparation{}, err
	}
	if schemaVersion != stateSchemaVersion {
		return preparation{}, newValueError("preparation schema must be 2")
	}
	return preparation{resource: resource, transactionID: transactionID, directory: directory}, nil
}

type requirement struct {
	path       string
	identifier string
}

func (r requirement) key() string {
	return pathKey(r.path) + "\x01" + r.identifier
}

type ownedClaim struct {
	claim              Claim
	claimID            string
	bundleID           string
	artifactID         string
	version            string
	bundleDigest       string
	renderDigest       string
	selection          *OwnedSelection
	installed          *revision
	disposition        Disposition
	baseline           *revision
	requirements       []requirement
	stableRequirements []requirement
	schemaRequirements []DocumentSchema
}

func newOwnedClaim(c ownedClaim) (ownedClaim, error) {
	if err := checkHexadecimal(c.claimID, 64); err != nil {
		return ownedClaim{}, err
	}
	if err := validateIdentifier(c.bundleID, "owner"); err != nil {
		return ownedClaim{}, err
	}
	if err := validateIdentifier(c.artifactID, "artifact"); err != nil {
		return ownedClaim{}, err
	}
	if err := validateVersion(c.version); err != nil {
		return ownedClaim{}, err
	}
	if err := checkHexadecimal(c.bundleDigest, 64); err != nil {
		return ownedClaim{}, err
	}
	if err := checkHexadecimal(c.renderDigest, 64); err != nil {
		return ownedClaim{}, err
	}
	if err := checkRequirements(c); err != nil {
		return ownedClaim{}, err
	}
	if err := checkOwnershipShape(c); err != nil {
		return ownedClaim{}, err
	}
	c.requirements = slices.Clone(c.requirements)
	c.stableRequirements = slices.Clone(c.stableRequirements)
	c.schemaRequirements = slices.Clone(c.schemaRequirements)
	return c, nil
}

func checkRequirements(c ownedClaim) error {
	for _, item := range c.requirements {
		if err := checkAbsolute(item.path); err != nil {
			return err
		}
		if err := checkHexadecimal(item.identifier, 64); err != nil {
			return err
		}
	}
	seen := make(map[string]bool, len(c.requirements))
	for _, item := range c.requirements {
		if seen[item.key()] {
			return newValueError("duplicate claim requirement")
		}
		seen[item.key()] = true
	}
	stable := make(map[string]bool, len(c.stableRequirements))
	for _, item := range c.stableRequirements {
		if stable[item.key()] || !seen[item.key()] {
			return newValueError("stable requirements must be a unique subset of requirements")
		}
		stable[item.key()] = true
	}
	keys := make(map[string]bool, len(c.schemaRequirements))
	for _, schema := range c.schemaRequirements {
		if keys[schema.key] {
			return newValueError("duplicate claim schema requirement")
		}
		keys[schema.key] = true
	}
	if len(c.schemaRequirements) > 0 && (c.selection == nil || slices.ContainsFunc(c.schemaRequirements, func(schema DocumentSchema) bool {
		return schema.format != c.selection.format
	})) {
		return newValueError("schema requirements need matching document selections")
	}
	return nil
}

func checkOwnershipShape(c ownedClaim) error {
	if c.claim.kind == OwnershipSection || c.claim.kind == OwnershipStructured {
		if c.selection == nil || !c.selection.claim.Equal(c.claim) {
			return newValueError("shared claims require matching selection ownership")
		}
		if c.installed != nil || c.baseline != nil {
			return newValueError("shared claims cannot own complete revisions")
		}
		if c.disposition != c.selection.disposition {
			return newValueError("selection disposition disagrees")
		}
		return nil
	}
	if c.selection != nil || c.installed == nil {
		return newValueError("whole claims require an installed revision")
	}
	if len(c.installed.nodes) == 0 {
		return newValueError("owned revisions cannot be absent")
	}
	if (c.disposition == DispositionTakenOver) != (c.baseline != nil) {
		return newValueError("only takeover has a baseline")
	}
	if c.baseline != nil && len(c.baseline.nodes) == 0 {
		return newValueError("takeover needs an original revision")
	}
	return nil
}

func optionalRevisionsEqual(a, b *revision) bool {
	if a == nil || b == nil {
		return a == b
	}
	return a.equal(*b)
}

func requirementsEqual(a, b []requirement) bool {
	return slices.EqualFunc(a, b, func(x, y requirement) bool {
		return pathsEqual(x.path, y.path) && x.identifier == y.identifier
	})
}

func (c ownedClaim) equal(o ownedClaim) bool {
	sameSelection := c.selection == nil && o.selection == nil ||
		c.selection != nil && o.selection != nil && c.selection.equal(*o.selection)
	return c.claim.Equal(o.claim) && c.claimID == o.claimID && c.bundleID == o.bundleID &&
		c.artifactID == o.artifactID && c.version == o.version && c.bundleDigest == o.bundleDigest &&
		c.renderDigest == o.renderDigest && sameSelection && optionalRevisionsEqual(c.installed, o.installed) &&
		c.disposition == o.disposition && optionalRevisionsEqual(c.baseline, o.baseline) &&
		requirementsEqual(c.requirements, o.requirements) && requirementsEqual(c.stableRequirements, o.stableRequirements) &&
		schemasEqual(c.schemaRequirements, o.schemaRequirements)
}

type receipt struct {
	resource           ResourceRef
	transactionID      string
	claims             []ownedClaim
	provenance         DocumentProvenance
	createdDirectories []node
}

func newReceipt(resource ResourceRef, transactionID string, claims []ownedClaim, provenance DocumentProvenance, createdDirectories []node, schemaVersion int64) (receipt, error) {
	if schemaVersion != stateSchemaVersion {
		return receipt{}, newValueError("receipt schema must be 2")
	}
	if err := checkHexadecimal(transactionID, 32); err != nil {
		return receipt{}, err
	}
	if len(createdDirectories) > 0 && resource.Kind != ResourceSkillContainer {
		return receipt{}, newValueError("directory provenance requires a skill container")
	}
	directories := make(map[string]bool, len(createdDirectories))
	for _, item := range createdDirectories {
		if item.hasData || directories[item.path] {
			return receipt{}, newValueError("created directory evidence must contain distinct directories")
		}
		directories[item.path] = true
	}
	allowed := map[ResourceKind][]Ownership{
		ResourceDocument:       {OwnershipFile, OwnershipSection, OwnershipStructured},
		ResourceTree:           {OwnershipTree},
		ResourceSkillContainer: {OwnershipSubtree},
	}[resource.Kind]
	for index, owned := range claims {
		identity, err := owned.claim.identity(resource.Destination)
		if err != nil {
			return receipt{}, err
		}
		if owned.claimID != identity {
			return receipt{}, newValueError("claim identity disagrees with physical destination")
		}
		if slices.ContainsFunc(claims[:index], func(other ownedClaim) bool { return owned.claim.Overlaps(other.claim) }) {
			return receipt{}, newValueError("receipt claims overlap")
		}
		if !slices.Contains(allowed, owned.claim.kind) {
			return receipt{}, newValueError("claim kind disagrees with resource kind")
		}
	}
	if err := checkReceiptSchemas(claims); err != nil {
		return receipt{}, err
	}
	return receipt{
		resource:           resource,
		transactionID:      transactionID,
		claims:             slices.Clone(claims),
		provenance:         provenance,
		createdDirectories: slices.Clone(createdDirectories),
	}, nil
}

func checkReceiptSchemas(claims []ownedClaim) error {
	var schemas []DocumentSchema
	for _, owned := range claims {
		for _, schema := range owned.schemaRequirements {
			if !slices.ContainsFunc(schemas, func(seen DocumentSchema) bool { return schemasEqual([]DocumentSchema{seen}, []DocumentSchema{schema}) }) {
				schemas = append(schemas, schema)
			}
		}
	}
	keys := make(map[string]bool, len(schemas))
	for _, schema := range schemas {
		if keys[schema.key] {
			return newValueError("receipt has incompatible schema requirements")
		}
		keys[schema.key] = true
	}
	for _, schema := range schemas {
		content, err := schema.Content()
		if err != nil {
			return err
		}
		claim, err := ContentClaim(content)
		if err != nil {
			return err
		}
		if slices.ContainsFunc(claims, func(owned ownedClaim) bool { return claim.Overlaps(owned.claim) }) {
			return newValueError("schema requirement overlaps app ownership")
		}
	}
	return nil
}

func (r receipt) equivalentExceptTransaction(o receipt) bool {
	return resourceRefsEqual(r.resource, o.resource) && slices.EqualFunc(r.claims, o.claims, ownedClaim.equal) &&
		r.provenance.equal(o.provenance) && slices.EqualFunc(r.createdDirectories, o.createdDirectories, node.equal)
}

type membership struct {
	resource ResourceRef
	claimIDs []string
}

type generation struct {
	version      string
	bundleDigest string
	renderDigest string
	membership   []membership
}

func newGeneration(version, bundleDigest, renderDigest string, members []membership) (generation, error) {
	if err := validateVersion(version); err != nil {
		return generation{}, err
	}
	if err := checkHexadecimal(bundleDigest, 64); err != nil {
		return generation{}, err
	}
	if err := checkHexadecimal(renderDigest, 64); err != nil {
		return generation{}, err
	}
	for _, member := range members {
		for _, identifier := range member.claimIDs {
			if err := checkHexadecimal(identifier, 64); err != nil {
				return generation{}, err
			}
		}
		seen := make(map[string]bool, len(member.claimIDs))
		for _, identifier := range member.claimIDs {
			if seen[identifier] {
				return generation{}, newValueError("duplicate membership claim")
			}
			seen[identifier] = true
		}
	}
	destinations := make(map[string]bool, len(members))
	for _, member := range members {
		key := pathKey(member.resource.Destination)
		if destinations[key] {
			return generation{}, newValueError("duplicate generation resource")
		}
		destinations[key] = true
	}
	copied := make([]membership, 0, len(members))
	for _, member := range members {
		copied = append(copied, membership{resource: member.resource, claimIDs: slices.Clone(member.claimIDs)})
	}
	return generation{version: version, bundleDigest: bundleDigest, renderDigest: renderDigest, membership: copied}, nil
}

type index struct {
	bundleID      string
	contextDigest string
	current       *generation
	pending       *generation
	previous      *generation
	residual      []ResourceRef
}

func newIndex(bundleID, contextDigest string, current, pending, previous *generation, residual []ResourceRef, schemaVersion int64) (index, error) {
	if err := validateIdentifier(bundleID, "index owner"); err != nil {
		return index{}, err
	}
	if err := checkHexadecimal(contextDigest, 64); err != nil {
		return index{}, err
	}
	if schemaVersion != stateSchemaVersion {
		return index{}, newValueError("index schema must be 2")
	}
	result := index{
		bundleID:      bundleID,
		contextDigest: contextDigest,
		current:       current,
		pending:       pending,
		previous:      previous,
		residual:      slices.Clone(residual),
	}
	resources := result.resources()
	destinations := make(map[string]bool, len(resources))
	for _, reference := range resources {
		key := pathKey(reference.Destination)
		if destinations[key] {
			return index{}, newValueError("index has conflicting resource kinds")
		}
		destinations[key] = true
	}
	return result, nil
}

func (x index) resources() []ResourceRef {
	references := slices.Clone(x.residual)
	seen := func(reference ResourceRef) bool {
		return slices.ContainsFunc(references, func(other ResourceRef) bool { return resourceRefsEqual(other, reference) })
	}
	for _, current := range []*generation{x.current, x.pending, x.previous} {
		if current == nil {
			continue
		}
		for _, member := range current.membership {
			if !seen(member.resource) {
				references = append(references, member.resource)
			}
		}
	}
	slices.SortStableFunc(references, func(a, b ResourceRef) int {
		return strings.Compare(posixPath(a.Destination), posixPath(b.Destination))
	})
	return references
}

type journal struct {
	resource  ResourceRef
	previous  *receipt
	receipt   receipt
	before    revision
	staged    revision
	published revision
	backup    revision
	ancestors []ancestor
}

func newJournal(j journal, schemaVersion int64) (journal, error) {
	if !resourceRefsEqual(j.receipt.resource, j.resource) || j.previous != nil && !resourceRefsEqual(j.previous.resource, j.resource) {
		return journal{}, newValueError("journal receipt resources disagree")
	}
	if schemaVersion != stateSchemaVersion {
		return journal{}, newValueError("journal schema must be 2")
	}
	if j.before.contentDigest() != j.backup.contentDigest() || j.staged.contentDigest() != j.published.contentDigest() {
		return journal{}, newValueError("journal private revisions must retain complete content")
	}
	j.ancestors = slices.Clone(j.ancestors)
	return j, nil
}

func indexKey(bundleID, contextDigest string) string {
	return "index/" + bundleID + "/" + contextDigest
}

func destinationKey(destination string) string {
	return sha256Hex([]byte(portablePathKey(unicodex.Lossy(destination))))
}

func resourceKey(destination, record string) string {
	return "resource/" + destinationKey(destination) + "/" + record
}
