package flyrail

import (
	"crypto/sha256"
	"encoding/hex"
	"slices"
	"strings"
	"unicode/utf8"

	"github.com/nielsmadan/flyrail/go/internal/pathx"
)

// NoticeKind classifies a rendering notice.
type NoticeKind string

// The notice kinds.
const (
	NoticePrerequisite NoticeKind = "prerequisite"
	NoticeActivation   NoticeKind = "activation"
	NoticeUnsupported  NoticeKind = "unsupported"
)

// ParseNoticeKind returns the notice kind spelled s.
func ParseNoticeKind(s string) (NoticeKind, error) {
	return parseEnum(s, "NoticeKind", NoticePrerequisite, NoticeActivation, NoticeUnsupported)
}

// Notice tells the caller about a prerequisite, an activation step or an unsupported request for one artifact.
type Notice struct {
	ArtifactID string
	Kind       NoticeKind
	Message    string
	Code       string // "" = none
}

func (n Notice) check() error {
	if err := validateIdentifier(n.ArtifactID, "notice artifact id"); err != nil {
		return err
	}
	if err := checkEnum(n.Kind, "NoticeKind", NoticePrerequisite, NoticeActivation, NoticeUnsupported); err != nil {
		return err
	}
	if !utf8.ValidString(n.Message) || strings.TrimSpace(n.Message) == "" {
		return newValueError("notice message must be nonblank")
	}
	if n.Code != "" {
		return validateIdentifier(n.Code, "notice code")
	}
	return nil
}

func blocking(notices []Notice) bool {
	return slices.ContainsFunc(notices, func(notice Notice) bool { return notice.Kind == NoticeUnsupported })
}

// RenderedArtifactParams are the inputs of NewRenderedArtifact.
type RenderedArtifactParams struct {
	ID                 string           // required
	Family             Family           // required
	Destination        string           // required, absolute
	Content            Content          // required
	Subtree            string           // "" = none; tree content only
	RequireExisting    bool             // false = the destination may be created
	SchemaRequirements []DocumentSchema // nil = none
}

// RenderedArtifact is one artifact routed to an absolute destination for one audience.
type RenderedArtifact struct {
	id                 string
	family             Family
	destination        string
	content            Content
	subtree            string
	requireExisting    bool
	schemaRequirements []DocumentSchema
}

// NewRenderedArtifact returns a validated rendered artifact.
func NewRenderedArtifact(p RenderedArtifactParams) (RenderedArtifact, error) {
	if err := validateIdentifier(p.ID, "rendered artifact id"); err != nil {
		return RenderedArtifact{}, err
	}
	if err := checkFamily(p.Family); err != nil {
		return RenderedArtifact{}, err
	}
	if !pathx.IsAbsolute(p.Destination) || !hasParent(p.Destination) || slices.Contains(pathx.Components(p.Destination), "..") {
		return RenderedArtifact{}, newValueError("rendered destination must be an absolute Path")
	}
	if err := checkContent(p.Content); err != nil {
		return RenderedArtifact{}, err
	}
	keys := make(map[string]struct{}, len(p.SchemaRequirements))
	for _, schema := range p.SchemaRequirements {
		if schema.format == "" {
			return RenderedArtifact{}, newTypeError("schema requirements must contain DocumentSchema values")
		}
		if _, duplicate := keys[schema.key]; duplicate {
			return RenderedArtifact{}, newValueError("duplicate document schema requirement")
		}
		keys[schema.key] = struct{}{}
	}
	if len(p.SchemaRequirements) > 0 {
		structured, ok := p.Content.(StructuredContent)
		if !ok || slices.ContainsFunc(p.SchemaRequirements, func(schema DocumentSchema) bool { return schema.format != structured.format }) {
			return RenderedArtifact{}, newValueError("document schema requirements need matching structured content")
		}
	}
	if p.Subtree != "" {
		if err := validateRelativePath(p.Subtree, "rendered subtree"); err != nil {
			return RenderedArtifact{}, err
		}
		if _, ok := p.Content.(TreeContent); !ok {
			return RenderedArtifact{}, newValueError("subtree claims require tree content")
		}
	}
	return RenderedArtifact{
		id:                 p.ID,
		family:             p.Family,
		destination:        p.Destination,
		content:            p.Content,
		subtree:            p.Subtree,
		requireExisting:    p.RequireExisting,
		schemaRequirements: slices.Clone(p.SchemaRequirements),
	}, nil
}

// ID returns the artifact id.
func (a RenderedArtifact) ID() string { return a.id }

// Family returns the artifact family.
func (a RenderedArtifact) Family() Family { return a.family }

// Destination returns the absolute destination path.
func (a RenderedArtifact) Destination() string { return a.destination }

// Content returns the payload.
func (a RenderedArtifact) Content() Content { return a.content }

// Subtree returns the claimed subtree of a skill container, or "" for none.
func (a RenderedArtifact) Subtree() string { return a.subtree }

// RequireExisting reports whether the destination must already exist.
func (a RenderedArtifact) RequireExisting() bool { return a.requireExisting }

// SchemaRequirements returns a copy of the document fields the destination must hold.
func (a RenderedArtifact) SchemaRequirements() []DocumentSchema {
	return slices.Clone(a.schemaRequirements)
}

func (a RenderedArtifact) params() RenderedArtifactParams {
	return RenderedArtifactParams{
		ID:                 a.id,
		Family:             a.family,
		Destination:        a.destination,
		Content:            a.content,
		Subtree:            a.subtree,
		RequireExisting:    a.requireExisting,
		SchemaRequirements: slices.Clone(a.schemaRequirements),
	}
}

func (a RenderedArtifact) modelValue() Value {
	subtree := Value(Null{})
	if a.subtree != "" {
		subtree = modelText(a.subtree)
	}
	return typedRecord("RenderedArtifact",
		field("content", modelContent(a.content)),
		field("destination", modelText(posixPath(a.destination))),
		field("family", modelText(string(a.family))),
		field("id", modelText(a.id)),
		field("require_existing", Bool(a.requireExisting)),
		field("schema_requirements", modelList(a.schemaRequirements, DocumentSchema.modelValue)),
		field("subtree", subtree),
	)
}

// RenderedBundle is the validated, dependency-ordered output of rendering a bundle for one or more contexts.
type RenderedBundle struct {
	artifacts     []RenderedArtifact
	dependencies  []Dependency
	notices       []Notice
	contentDigest string
	routing       []contextEntry
}

// NewRenderedBundle returns a rendered bundle of artifacts, dependencies, notices and routing context; nil notices means none.
func NewRenderedBundle(artifacts []RenderedArtifact, deps []Dependency, notices []Notice, routing map[string]string) (*RenderedBundle, error) {
	return newRenderedBundle(artifacts, deps, notices, contextEntries(routing))
}

func newRenderedBundle(artifacts []RenderedArtifact, deps []Dependency, notices []Notice, routing []contextEntry) (*RenderedBundle, error) {
	if slices.ContainsFunc(artifacts, func(artifact RenderedArtifact) bool { return artifact.id == "" }) {
		return nil, newTypeError("rendered artifacts must contain RenderedArtifact values")
	}
	for _, notice := range notices {
		if err := notice.check(); err != nil {
			return nil, err
		}
	}
	if !validContextEntries(routing) {
		return nil, newTypeError("render routing context must contain string pairs")
	}
	names := make([]string, len(artifacts))
	byID := make(map[string]RenderedArtifact, len(artifacts))
	for index, artifact := range artifacts {
		names[index] = artifact.id
		byID[artifact.id] = artifact
	}
	if len(byID) != len(artifacts) {
		return nil, newValueError("rendered artifact ids must be unique")
	}
	if _, err := DependencyOrder(names, deps); err != nil {
		return nil, err
	}
	for _, edge := range deps {
		if edge.mode != DependencyStableReference {
			continue
		}
		if _, file := byID[edge.required].content.(FileContent); !file {
			return nil, newValueError("stable references require a whole-file routing entry")
		}
		if slices.ContainsFunc(deps, func(child Dependency) bool {
			return child.dependent == edge.required && child.mode != DependencyRevision
		}) {
			return nil, newValueError("stable routing entries require revision-bound dependencies")
		}
	}
	sortedArtifacts := slices.Clone(artifacts)
	slices.SortStableFunc(sortedArtifacts, func(a, b RenderedArtifact) int { return strings.Compare(a.id, b.id) })
	sortedDeps := slices.Clone(deps)
	slices.SortStableFunc(sortedDeps, compareDependencies)
	return &RenderedBundle{
		artifacts:     sortedArtifacts,
		dependencies:  sortedDeps,
		notices:       slices.Clone(notices),
		contentDigest: renderDigest(sortedArtifacts, sortedDeps),
		routing:       slices.Clone(routing),
	}, nil
}

func compareDependencies(a, b Dependency) int {
	if order := strings.Compare(a.dependent, b.dependent); order != 0 {
		return order
	}
	return strings.Compare(a.required, b.required)
}

// EmptyRenderedBundle returns a rendered bundle with no artifacts, dependencies, notices or routing context.
func EmptyRenderedBundle() *RenderedBundle {
	return &RenderedBundle{contentDigest: renderDigest(nil, nil)}
}

// Artifacts returns the artifacts in id order.
func (r *RenderedBundle) Artifacts() []RenderedArtifact {
	if r == nil {
		return nil
	}
	return slices.Clone(r.artifacts)
}

// Dependencies returns the dependencies in dependent, required order.
func (r *RenderedBundle) Dependencies() []Dependency {
	if r == nil {
		return nil
	}
	return slices.Clone(r.dependencies)
}

// Notices returns the notices in emission order.
func (r *RenderedBundle) Notices() []Notice {
	if r == nil {
		return nil
	}
	return slices.Clone(r.notices)
}

// Supported reports whether no notice marks the rendering unsupported.
func (r *RenderedBundle) Supported() bool {
	return r != nil && !blocking(r.notices)
}

// ContentDigest returns the SHA-256 hex digest of the canonical artifacts and dependencies.
func (r *RenderedBundle) ContentDigest() string {
	if r == nil {
		return ""
	}
	return r.contentDigest
}

// RoutingContext returns a copy of the routing context.
func (r *RenderedBundle) RoutingContext() map[string]string {
	if r == nil {
		return map[string]string{}
	}
	return contextMap(r.routing)
}

func renderedModel(artifacts []RenderedArtifact, deps []Dependency) Value {
	return modelArray([]Value{
		modelList(artifacts, RenderedArtifact.modelValue),
		modelList(deps, Dependency.modelValue),
	})
}

func renderDigest(artifacts []RenderedArtifact, deps []Dependency) string {
	semantic, _ := EncodeValue(renderedModel(artifacts, deps))
	hash := sha256.New()
	hash.Write([]byte("flyrail-render-v1\x00"))
	hash.Write(semantic)
	return hex.EncodeToString(hash.Sum(nil))
}
