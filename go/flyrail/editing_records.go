package flyrail

import (
	"slices"
	"strings"
)

// Disposition is how an owned selection was acquired.
type Disposition string

// The dispositions, spelled as on the wire.
const (
	DispositionCreated   Disposition = "created"
	DispositionTakenOver Disposition = "taken-over"
	DispositionAdopted   Disposition = "adopted"
)

// ParseDisposition returns the Disposition spelled s.
func ParseDisposition(s string) (Disposition, error) {
	return parseEnum(s, "Disposition", DispositionCreated, DispositionTakenOver, DispositionAdopted)
}

// SnapshotValue is the recorded value of a selection: SnapshotBytes for text sections, SnapshotTyped for structured documents.
type SnapshotValue interface {
	isSnapshotValue()
}

// SnapshotBytes is the raw text of a section selection.
type SnapshotBytes struct {
	data []byte
}

// SnapshotTyped is the semantic value of a structured selection.
type SnapshotTyped struct {
	value Value
}

func (SnapshotBytes) isSnapshotValue() {}
func (SnapshotTyped) isSnapshotValue() {}

// NewSnapshotBytes returns a snapshot of section text.
func NewSnapshotBytes(b []byte) SnapshotBytes {
	return SnapshotBytes{data: cloneBytes(b)}
}

// Bytes returns a copy of the section text.
func (s SnapshotBytes) Bytes() []byte { return cloneBytes(s.data) }

// NewSnapshotTyped returns a snapshot of a structured value.
func NewSnapshotTyped(v Value) (SnapshotTyped, error) {
	if _, err := checkValue(v); err != nil {
		return SnapshotTyped{}, err
	}
	return SnapshotTyped{value: v}, nil
}

// Value returns the structured value.
func (s SnapshotTyped) Value() Value { return s.value }

func cloneBytes(b []byte) []byte {
	return append([]byte{}, b...)
}

func snapshotValuesEqual(a, b SnapshotValue) bool {
	switch left := a.(type) {
	case SnapshotBytes:
		right, ok := b.(SnapshotBytes)
		return ok && string(left.data) == string(right.data)
	case SnapshotTyped:
		right, ok := b.(SnapshotTyped)
		return ok && ValuesEqual(left.value, right.value)
	}
	return b == nil
}

// ArrayPosition records the neighbours and leading trivia of an owned array member.
type ArrayPosition struct {
	previous  *Member
	following *Member
	prefix    []byte
}

// NewArrayPosition returns an array position; nil anchors mean none.
func NewArrayPosition(previous, following *Member, prefix []byte) (ArrayPosition, error) {
	position := ArrayPosition{prefix: cloneBytes(prefix)}
	for _, anchor := range []*Member{previous, following} {
		if anchor != nil {
			if err := anchor.valid(); err != nil {
				return ArrayPosition{}, err
			}
		}
	}
	if previous != nil {
		copied := *previous
		position.previous = &copied
	}
	if following != nil {
		copied := *following
		position.following = &copied
	}
	if previous != nil && following != nil && previous.equal(*following) {
		return ArrayPosition{}, newValueError("array position anchors must be distinct")
	}
	return position, nil
}

// Previous returns the member before the owned one, if any.
func (p ArrayPosition) Previous() (Member, bool) {
	if p.previous == nil {
		return Member{}, false
	}
	return *p.previous, true
}

// Following returns the member after the owned one, if any.
func (p ArrayPosition) Following() (Member, bool) {
	if p.following == nil {
		return Member{}, false
	}
	return *p.following, true
}

// Prefix returns a copy of the trivia before the owned member.
func (p ArrayPosition) Prefix() []byte { return cloneBytes(p.prefix) }

func optionalMembersEqual(a, b *Member) bool {
	if a == nil || b == nil {
		return a == b
	}
	return a.equal(*b)
}

func (p ArrayPosition) equal(o ArrayPosition) bool {
	return optionalMembersEqual(p.previous, o.previous) && optionalMembersEqual(p.following, o.following) &&
		string(p.prefix) == string(o.prefix)
}

// SelectionSnapshot is the recorded value and syntax of one owned selection.
type SelectionSnapshot struct {
	value       SnapshotValue
	syntax      []byte
	ownedSyntax []byte
	position    *ArrayPosition
	layout      []byte
}

// NewSelectionSnapshot returns a snapshot; a nil position means none, and layout is TOML restoration evidence.
func NewSelectionSnapshot(value SnapshotValue, syntax, ownedSyntax []byte, position *ArrayPosition, layout []byte) (SelectionSnapshot, error) {
	switch v := value.(type) {
	case SnapshotBytes:
		value = NewSnapshotBytes(v.data)
	case SnapshotTyped:
		if _, err := checkValue(v.value); err != nil {
			return SelectionSnapshot{}, err
		}
	default:
		return SelectionSnapshot{}, newTypeError("selection snapshot value must be bytes or a Value")
	}
	snapshot := SelectionSnapshot{
		value:       value,
		syntax:      cloneBytes(syntax),
		ownedSyntax: cloneBytes(ownedSyntax),
		layout:      cloneBytes(layout),
	}
	if position != nil {
		copied := ArrayPosition{previous: position.previous, following: position.following, prefix: cloneBytes(position.prefix)}
		snapshot.position = &copied
	}
	if err := snapshot.validateLayouts(); err != nil {
		return SelectionSnapshot{}, err
	}
	return snapshot, nil
}

func (s SelectionSnapshot) validateLayouts() error {
	headers, err := layoutHeaders(s.layout)
	if err != nil {
		return err
	}
	for _, header := range headers {
		var current Value
		if typed, ok := s.value.(SnapshotTyped); ok {
			current = typed.value
		}
		for _, part := range header.path {
			current = layoutStep(current, part)
			if current == nil {
				return newValueError("invalid TOML restoration path")
			}
		}
		if _, ok := current.(Object); !ok {
			return newValueError("TOML restoration header requires an object")
		}
	}
	return nil
}

func layoutStep(current Value, part layoutPart) Value {
	switch value := current.(type) {
	case Object:
		if part.isIndex {
			return nil
		}
		item, _ := value.Get(part.field)
		return item
	case Array:
		if !part.isIndex || part.index >= uint64(len(value.items)) {
			return nil
		}
		return value.items[part.index]
	}
	return nil
}

// Value returns the recorded value.
func (s SelectionSnapshot) Value() SnapshotValue {
	if bytes, ok := s.value.(SnapshotBytes); ok {
		return NewSnapshotBytes(bytes.data)
	}
	return s.value
}

// Syntax returns a copy of the selection's full syntax.
func (s SelectionSnapshot) Syntax() []byte { return cloneBytes(s.syntax) }

// OwnedSyntax returns a copy of the syntax Flyrail owns.
func (s SelectionSnapshot) OwnedSyntax() []byte { return cloneBytes(s.ownedSyntax) }

// Layout returns a copy of the TOML restoration layout.
func (s SelectionSnapshot) Layout() []byte { return cloneBytes(s.layout) }

// Position returns the array position, if any.
func (s SelectionSnapshot) Position() (ArrayPosition, bool) {
	if s.position == nil {
		return ArrayPosition{}, false
	}
	return *s.position, true
}

func (s SelectionSnapshot) equal(o SelectionSnapshot) bool {
	samePosition := s.position == nil && o.position == nil ||
		s.position != nil && o.position != nil && s.position.equal(*o.position)
	return snapshotValuesEqual(s.value, o.value) && string(s.syntax) == string(o.syntax) &&
		string(s.ownedSyntax) == string(o.ownedSyntax) && samePosition && string(s.layout) == string(o.layout)
}

// OwnedSelectionParams are the inputs of NewOwnedSelection.
type OwnedSelectionParams struct {
	Claim       Claim              // required
	Format      DocumentFormat     // "" = section (text) claim; set for structured claims
	Installed   SelectionSnapshot  // required
	Disposition Disposition        // required
	Baseline    *SelectionSnapshot // nil = none; required exactly for DispositionTakenOver
	Separator   []byte             // nil = none; section claims only
}

// OwnedSelection is the record of one claim Flyrail owns inside a shared document.
type OwnedSelection struct {
	claim       Claim
	format      DocumentFormat
	installed   SelectionSnapshot
	disposition Disposition
	baseline    *SelectionSnapshot
	separator   []byte
}

// NewOwnedSelection returns a validated owned selection.
func NewOwnedSelection(p OwnedSelectionParams) (OwnedSelection, error) {
	if p.Claim.selector == nil {
		return OwnedSelection{}, newTypeError("ownership requires a Claim")
	}
	if p.Format != "" {
		if err := checkEnum(p.Format, "DocumentFormat", FormatJSON, FormatJSONC, FormatTOML); err != nil {
			return OwnedSelection{}, err
		}
	}
	if (p.Claim.kind == OwnershipSection) != (p.Format == "") || p.Claim.kind != OwnershipSection && p.Claim.kind != OwnershipStructured {
		return OwnedSelection{}, newValueError("shared claim and document format disagree")
	}
	if p.Installed.value == nil || checkEnum(p.Disposition, "Disposition", DispositionCreated, DispositionTakenOver, DispositionAdopted) != nil {
		return OwnedSelection{}, newTypeError("ownership requires an installed snapshot and disposition")
	}
	if (p.Disposition == DispositionTakenOver) != (p.Baseline != nil) {
		return OwnedSelection{}, newValueError("only takeover ownership has a baseline")
	}
	if p.Baseline != nil && p.Baseline.value == nil {
		return OwnedSelection{}, newTypeError("takeover baseline must be a SelectionSnapshot")
	}
	selection := OwnedSelection{
		claim:       p.Claim,
		format:      p.Format,
		installed:   p.Installed,
		disposition: p.Disposition,
		separator:   cloneBytes(p.Separator),
	}
	if p.Baseline != nil {
		baseline := *p.Baseline
		selection.baseline = &baseline
	}
	snapshots := []SelectionSnapshot{selection.installed}
	if selection.baseline != nil {
		snapshots = append(snapshots, *selection.baseline)
	}
	for _, snapshot := range snapshots {
		if err := selection.checkSnapshot(snapshot); err != nil {
			return OwnedSelection{}, err
		}
	}
	if selection.format != "" && len(selection.separator) > 0 {
		return OwnedSelection{}, newValueError("section separators require a text claim")
	}
	return selection, nil
}

func (s OwnedSelection) checkSnapshot(snapshot SelectionSnapshot) error {
	if _, isBytes := snapshot.value.(SnapshotBytes); isBytes != (s.format == "") {
		return newValueError("selection snapshot and document format disagree")
	}
	if len(snapshot.layout) == 0 {
		return nil
	}
	selector, structured := s.claim.selector.(Selector)
	if s.format != FormatTOML || !structured {
		return newValueError("restoration layout requires a TOML claim")
	}
	var path []string
	for _, part := range selector.parts {
		if key, ok := part.(Key); ok {
			path = append(path, key.name)
		}
	}
	headers, err := layoutHeaders(snapshot.layout)
	if err != nil {
		return err
	}
	for _, header := range headers {
		expected := slices.Clone(path)
		for _, part := range header.path {
			if !part.isIndex {
				expected = append(expected, part.field)
			}
		}
		names, err := headerPath(header.name)
		if err != nil {
			return err
		}
		if !slices.Equal(names, expected) {
			return newValueError("TOML restoration header does not match its destination")
		}
	}
	return nil
}

// Claim returns the owned claim.
func (s OwnedSelection) Claim() Claim { return s.claim }

// Format returns the document format, or "" for a section claim.
func (s OwnedSelection) Format() DocumentFormat { return s.format }

// Installed returns the snapshot Flyrail installed.
func (s OwnedSelection) Installed() SelectionSnapshot { return s.installed }

// Disposition returns how the selection was acquired.
func (s OwnedSelection) Disposition() Disposition { return s.disposition }

// Baseline returns the snapshot a takeover replaced, if any.
func (s OwnedSelection) Baseline() (SelectionSnapshot, bool) {
	if s.baseline == nil {
		return SelectionSnapshot{}, false
	}
	return *s.baseline, true
}

// Separator returns a copy of the separator Flyrail added before a section.
func (s OwnedSelection) Separator() []byte { return cloneBytes(s.separator) }

func (s OwnedSelection) equal(o OwnedSelection) bool {
	sameBaseline := s.baseline == nil && o.baseline == nil ||
		s.baseline != nil && o.baseline != nil && s.baseline.equal(*o.baseline)
	return s.claim.Equal(o.claim) && s.format == o.format && s.installed.equal(o.installed) &&
		s.disposition == o.disposition && sameBaseline && string(s.separator) == string(o.separator)
}

// CreatedContainer records an object or array Flyrail created to hold a selection.
type CreatedContainer struct {
	selector    Selector
	emptySyntax []byte
}

// NewCreatedContainer returns container provenance; it is validated where it is consumed.
func NewCreatedContainer(selector Selector, emptySyntax []byte) CreatedContainer {
	return CreatedContainer{selector: selector, emptySyntax: cloneBytes(emptySyntax)}
}

// Selector returns the container's selector.
func (c CreatedContainer) Selector() Selector { return c.selector }

// EmptySyntax returns a copy of the container's syntax when it was created empty.
func (c CreatedContainer) EmptySyntax() []byte { return cloneBytes(c.emptySyntax) }

// DocumentProvenanceParams are the inputs of NewDocumentProvenance.
type DocumentProvenanceParams struct {
	EmptyDocument          []byte             // used only when HasEmptyDocument
	HasEmptyDocument       bool               // false = Rust None
	Containers             []CreatedContainer // nil = none
	TrailingSeparators     []string           // nil = none; SHA-256 hex digests
	SchemaFields           []OwnedSelection   // nil = none
	SchemaEmptyDocument    []byte             // used only when HasSchemaEmptyDocument
	HasSchemaEmptyDocument bool               // false = Rust None
}

// DocumentProvenance records what Flyrail created in a shared document; the zero value records nothing.
type DocumentProvenance struct {
	emptyDocument       []byte
	hasEmptyDocument    bool
	containers          []CreatedContainer
	trailingSeparators  []string
	schemaFields        []OwnedSelection
	schemaEmptyDocument []byte
	hasSchemaEmpty      bool
}

// NewDocumentProvenance returns validated document provenance.
func NewDocumentProvenance(p DocumentProvenanceParams) (DocumentProvenance, error) {
	for _, item := range p.SchemaFields {
		if !schemaField(item) {
			return DocumentProvenance{}, newValueError("schema provenance requires created top-level scalar fields")
		}
	}
	for index, item := range p.SchemaFields {
		if slices.ContainsFunc(p.SchemaFields[:index], func(other OwnedSelection) bool { return other.claim.Equal(item.claim) }) {
			return DocumentProvenance{}, newValueError("duplicate schema provenance")
		}
	}
	if p.HasSchemaEmptyDocument && strings.Trim(string(p.SchemaEmptyDocument), " \t\n\r\v\f") != "" {
		return DocumentProvenance{}, newValueError("schema empty-document evidence must be whitespace bytes")
	}
	for _, container := range p.Containers {
		if len(container.selector.parts) == 0 {
			return DocumentProvenance{}, newTypeError("container provenance requires a selector and immutable syntax")
		}
	}
	for index, container := range p.Containers {
		if slices.ContainsFunc(p.Containers[:index], func(other CreatedContainer) bool {
			return selectorsEqual(other.selector, container.selector)
		}) {
			return DocumentProvenance{}, newValueError("duplicate container provenance")
		}
	}
	for _, digest := range p.TrailingSeparators {
		if !isSHA256Text(digest) {
			return DocumentProvenance{}, newValueError("separator evidence must be SHA-256 digests")
		}
	}
	provenance := DocumentProvenance{
		hasEmptyDocument:   p.HasEmptyDocument,
		trailingSeparators: slices.Clone(p.TrailingSeparators),
		schemaFields:       slices.Clone(p.SchemaFields),
		hasSchemaEmpty:     p.HasSchemaEmptyDocument,
	}
	if p.HasEmptyDocument {
		provenance.emptyDocument = cloneBytes(p.EmptyDocument)
	}
	if p.HasSchemaEmptyDocument {
		provenance.schemaEmptyDocument = cloneBytes(p.SchemaEmptyDocument)
	}
	for _, container := range p.Containers {
		provenance.containers = append(provenance.containers, NewCreatedContainer(container.selector, container.emptySyntax))
	}
	return provenance, nil
}

func schemaField(item OwnedSelection) bool {
	selector, ok := item.claim.selector.(Selector)
	if !ok || item.disposition != DispositionCreated || len(selector.parts) != 1 {
		return false
	}
	if _, isKey := selector.parts[0].(Key); !isKey || item.format != FormatJSON && item.format != FormatJSONC {
		return false
	}
	typed, ok := item.installed.value.(SnapshotTyped)
	if !ok {
		return false
	}
	_, scalar := typed.value.(Scalar)
	return scalar
}

func isSHA256Text(value string) bool {
	return len(value) == 64 && strings.IndexFunc(value, func(r rune) bool {
		return (r < '0' || r > '9') && (r < 'a' || r > 'f')
	}) < 0
}

// EmptyDocument returns the document Flyrail created empty, if any.
func (p DocumentProvenance) EmptyDocument() ([]byte, bool) {
	if !p.hasEmptyDocument {
		return nil, false
	}
	return cloneBytes(p.emptyDocument), true
}

// Containers returns the containers Flyrail created.
func (p DocumentProvenance) Containers() []CreatedContainer {
	containers := make([]CreatedContainer, 0, len(p.containers))
	for _, container := range p.containers {
		containers = append(containers, NewCreatedContainer(container.selector, container.emptySyntax))
	}
	return containers
}

// TrailingSeparators returns the digests of separators Flyrail added at the document end.
func (p DocumentProvenance) TrailingSeparators() []string { return slices.Clone(p.trailingSeparators) }

// SchemaFields returns the schema fields Flyrail created.
func (p DocumentProvenance) SchemaFields() []OwnedSelection { return slices.Clone(p.schemaFields) }

// SchemaEmptyDocument returns the whitespace document a schema scaffold replaced, if any.
func (p DocumentProvenance) SchemaEmptyDocument() ([]byte, bool) {
	if !p.hasSchemaEmpty {
		return nil, false
	}
	return cloneBytes(p.schemaEmptyDocument), true
}

func optionalBytesEqual(a []byte, hasA bool, b []byte, hasB bool) bool {
	return hasA == hasB && string(a) == string(b)
}

func (p DocumentProvenance) equal(o DocumentProvenance) bool {
	return optionalBytesEqual(p.emptyDocument, p.hasEmptyDocument, o.emptyDocument, o.hasEmptyDocument) &&
		slices.EqualFunc(p.containers, o.containers, func(a, b CreatedContainer) bool {
			return selectorsEqual(a.selector, b.selector) && string(a.emptySyntax) == string(b.emptySyntax)
		}) &&
		slices.Equal(p.trailingSeparators, o.trailingSeparators) &&
		slices.EqualFunc(p.schemaFields, o.schemaFields, OwnedSelection.equal) &&
		optionalBytesEqual(p.schemaEmptyDocument, p.hasSchemaEmpty, o.schemaEmptyDocument, o.hasSchemaEmpty)
}
