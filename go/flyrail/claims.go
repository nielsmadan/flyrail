package flyrail

import (
	"crypto/sha256"
	"encoding/hex"
	"slices"
	"strings"

	"github.com/nielsmadan/flyrail/go/internal/jsonx"
	"github.com/nielsmadan/flyrail/go/internal/pathx"
	"github.com/nielsmadan/flyrail/go/internal/unicodex"
)

// Ownership is the kind of a claim on a resource.
type Ownership string

// The ownership kinds, spelled as on the wire.
const (
	OwnershipFile       Ownership = "file"
	OwnershipTree       Ownership = "tree"
	OwnershipSubtree    Ownership = "subtree"
	OwnershipSection    Ownership = "section"
	OwnershipStructured Ownership = "structured"
)

// ParseOwnership returns the Ownership spelled s.
func ParseOwnership(s string) (Ownership, error) {
	return parseEnum(s, "Ownership", OwnershipFile, OwnershipTree, OwnershipSubtree, OwnershipSection, OwnershipStructured)
}

// ClaimSelector is the part of a resource a claim selects: WholeSelector, SubtreeSelector, SectionBoundaries or Selector.
type ClaimSelector interface {
	isClaimSelector()
}

// WholeSelector selects a whole file or tree.
type WholeSelector struct{}

// SubtreeSelector selects a relative subtree of a skill container, or names a section marker.
type SubtreeSelector string

func (WholeSelector) isClaimSelector()     {}
func (SubtreeSelector) isClaimSelector()   {}
func (SectionBoundaries) isClaimSelector() {}
func (Selector) isClaimSelector()          {}

// Claim is the part of one resource an artifact owns.
type Claim struct {
	_        [0]func()
	kind     Ownership
	selector ClaimSelector
}

// NewClaim returns a claim of kind on selector; a section claim given a SubtreeSelector marker converts it to boundaries.
func NewClaim(kind Ownership, selector ClaimSelector) (Claim, error) {
	if err := checkEnum(kind, "Ownership", OwnershipFile, OwnershipTree, OwnershipSubtree, OwnershipSection, OwnershipStructured); err != nil {
		return Claim{}, err
	}
	switch kind {
	case OwnershipFile, OwnershipTree:
		if _, ok := selector.(WholeSelector); !ok {
			return Claim{}, newValueError("whole-resource claims cannot have selectors")
		}
	case OwnershipStructured:
		structured, ok := selector.(Selector)
		if !ok {
			return Claim{}, newTypeError("structured claims require a Selector")
		}
		if len(structured.parts) == 0 {
			return Claim{}, newValueError("a selector must select a key or member")
		}
	case OwnershipSection:
		switch value := selector.(type) {
		case SubtreeSelector:
			boundaries, err := SectionBoundariesFromMarker(string(value))
			if err != nil {
				return Claim{}, err
			}
			selector = boundaries
		case SectionBoundaries:
			if value.start == "" {
				return Claim{}, newValueError("section boundaries must be nonblank single lines without NUL")
			}
		default:
			return Claim{}, newTypeError("section claims require SectionBoundaries or a marker name")
		}
	case OwnershipSubtree:
		path, ok := selector.(SubtreeSelector)
		if !ok {
			return Claim{}, newTypeError("subtree claims require a string selector")
		}
		if err := validateRelativePath(string(path), "subtree selector"); err != nil {
			return Claim{}, err
		}
	}
	return Claim{kind: kind, selector: selector}, nil
}

// Kind returns the ownership kind.
func (c Claim) Kind() Ownership { return c.kind }

// Selector returns the selected part of the resource.
func (c Claim) Selector() ClaimSelector { return c.selector }

// Equal reports whether c and o claim the same part of a resource.
func (c Claim) Equal(o Claim) bool {
	if c.kind != o.kind {
		return false
	}
	switch left := c.selector.(type) {
	case WholeSelector:
		_, ok := o.selector.(WholeSelector)
		return ok
	case SubtreeSelector:
		right, ok := o.selector.(SubtreeSelector)
		return ok && left == right
	case SectionBoundaries:
		right, ok := o.selector.(SectionBoundaries)
		return ok && left == right
	case Selector:
		right, ok := o.selector.(Selector)
		return ok && selectorsEqual(left, right)
	}
	return o.selector == nil
}

// Overlaps reports whether c and o could select overlapping parts of one resource.
func (c Claim) Overlaps(o Claim) bool {
	whole := func(kind Ownership) bool { return kind == OwnershipFile || kind == OwnershipTree }
	if whole(c.kind) || whole(o.kind) || c.kind != o.kind {
		return true
	}
	switch left := c.selector.(type) {
	case SectionBoundaries:
		right, ok := o.selector.(SectionBoundaries)
		if !ok {
			return true
		}
		bounds := []string{right.start, right.end}
		return slices.Contains(bounds, left.start) || slices.Contains(bounds, left.end)
	case Selector:
		right, ok := o.selector.(Selector)
		if !ok {
			return true
		}
		return structuredOverlap(left, right)
	case SubtreeSelector:
		right, ok := o.selector.(SubtreeSelector)
		if !ok {
			return true
		}
		leftKey, rightKey := portablePathKey(string(left)), portablePathKey(string(right))
		return leftKey == rightKey || strings.HasPrefix(leftKey, rightKey+"/") || strings.HasPrefix(rightKey, leftKey+"/")
	}
	return true
}

func structuredOverlap(left, right Selector) bool {
	for index := range min(len(left.parts), len(right.parts)) {
		leftPart, rightPart := left.parts[index], right.parts[index]
		if selectorPartsEqual(leftPart, rightPart) {
			continue
		}
		_, leftIsKey := leftPart.(Key)
		_, rightIsKey := rightPart.(Key)
		leftMember, leftIsMember := leftPart.(Member)
		rightMember, rightIsMember := rightPart.(Member)
		switch {
		case leftIsKey && rightIsKey:
			return false
		case leftIsMember && rightIsMember:
			return !slices.Equal(leftMember.key, rightMember.key) || ValuesEqual(leftMember.identity, rightMember.identity)
		}
		return true
	}
	return true
}

func (c Claim) identity(destination string) (string, error) {
	var selected jsonx.Value
	switch selector := c.selector.(type) {
	case WholeSelector:
		selected = jsonx.Null()
	case SubtreeSelector:
		selected = jsonx.String(string(selector))
	case SectionBoundaries:
		selected = jsonx.Array(jsonx.String(selector.start), jsonx.String(selector.end))
	case Selector:
		parts := make([]jsonx.Value, 0, len(selector.parts))
		for _, part := range selector.parts {
			parts = append(parts, partRecord(part))
		}
		selected = jsonx.Array(parts...)
	default:
		return "", newValueError("claim has no selector")
	}
	record := jsonx.Array(jsonx.String(posixPath(destination)), jsonx.String(string(c.kind)), selected)
	value, err := valueFromJSON(record)
	if err != nil {
		return "", err
	}
	semantic, err := EncodeValue(value)
	if err != nil {
		return "", err
	}
	digest := sha256.New()
	digest.Write([]byte("flyrail-claim-v1\x00"))
	digest.Write(semantic)
	return hex.EncodeToString(digest.Sum(nil)), nil
}

func partRecord(part SelectorPart) jsonx.Value {
	switch value := part.(type) {
	case Key:
		return jsonx.Array(jsonx.String("key"), jsonx.String(value.name))
	case Member:
		names := make([]jsonx.Value, 0, len(value.key))
		for _, name := range value.key {
			names = append(names, jsonx.String(name))
		}
		return jsonx.Array(jsonx.String("member"), jsonx.Array(names...), semanticRecord(value.identity))
	}
	return jsonx.Null()
}

// ContentClaim returns the claim a section or structured content addresses.
func ContentClaim(c Content) (Claim, error) {
	switch content := c.(type) {
	case SectionContent:
		if err := checkContent(content); err != nil {
			return Claim{}, err
		}
		return NewClaim(OwnershipSection, content.Selector())
	case StructuredContent:
		if err := checkContent(content); err != nil {
			return Claim{}, err
		}
		return NewClaim(OwnershipStructured, content.selector)
	}
	return Claim{}, newTypeError("document content must be a section or structured selection")
}

func sha256Hex(data []byte) string {
	sum := sha256.Sum256(data)
	return hex.EncodeToString(sum[:])
}

func nameKey(destination string) string {
	name := ""
	if components := pathx.Components(destination); len(components) > 0 {
		last := components[len(components)-1]
		if last != "/" && last != "." {
			name = unicodex.Lossy(last)
		}
	}
	return sha256Hex([]byte(portablePathKey(name)))
}

// TransactionPath returns the private transaction directory beside destination for transactionID.
func TransactionPath(destination, transactionID string) (string, error) {
	if len(transactionID) != 32 || strings.IndexFunc(transactionID, func(r rune) bool {
		return (r < '0' || r > '9') && (r < 'a' || r > 'f')
	}) >= 0 {
		return "", newValueError("transaction id must be 32 lowercase hexadecimal characters")
	}
	name := ".flyrail-" + nameKey(destination) + "-" + transactionID + ".txn"
	if parent, ok := pathx.Parent(destination); ok {
		return pathx.Join(parent, name), nil
	}
	if pathx.IsAbsolute(destination) {
		return "/" + name, nil
	}
	return name, nil
}
