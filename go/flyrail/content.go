package flyrail

import (
	"slices"
	"strings"
	"unicode/utf8"

	"github.com/nielsmadan/flyrail/go/internal/unicodex"
)

// DocumentFormat is the syntax of a structured configuration document.
type DocumentFormat string

// The document formats.
const (
	FormatJSON  DocumentFormat = "json"
	FormatJSONC DocumentFormat = "jsonc"
	FormatTOML  DocumentFormat = "toml"
)

// ParseDocumentFormat returns the document format spelled s.
func ParseDocumentFormat(s string) (DocumentFormat, error) {
	return parseEnum(s, "DocumentFormat", FormatJSON, FormatJSONC, FormatTOML)
}

// FileMode is the permission class of a managed file.
type FileMode string

// The file modes.
const (
	ModePrivate    FileMode = "private"
	ModeReadable   FileMode = "readable"
	ModeExecutable FileMode = "executable"
)

// ParseFileMode returns the file mode spelled s.
func ParseFileMode(s string) (FileMode, error) {
	return parseEnum(s, "FileMode", ModePrivate, ModeReadable, ModeExecutable)
}

// Position is where a structured array member is inserted.
type Position string

// The insertion positions.
const (
	PositionFirst  Position = "first"
	PositionLast   Position = "last"
	PositionBefore Position = "before"
	PositionAfter  Position = "after"
)

// ParsePosition returns the position spelled s.
func ParsePosition(s string) (Position, error) {
	return parseEnum(s, "Position", PositionFirst, PositionLast, PositionBefore, PositionAfter)
}

func validText(value, label string) error {
	if !utf8.ValidString(value) {
		return newValueError(label + " must contain Unicode scalar values")
	}
	return nil
}

// Content is the payload of a native or rendered artifact: FileContent, TreeContent, SectionContent or StructuredContent.
type Content interface {
	isContent()
}

func (FileContent) isContent()       {}
func (TreeContent) isContent()       {}
func (SectionContent) isContent()    {}
func (StructuredContent) isContent() {}

func checkContent(content Content) error {
	switch c := content.(type) {
	case FileContent:
		return checkEnum(c.mode, "FileMode", ModePrivate, ModeReadable, ModeExecutable)
	case TreeContent:
		return nil
	case SectionContent:
		return validateIdentifier(c.marker, "section marker")
	case StructuredContent:
		return checkEnum(c.format, "DocumentFormat", FormatJSON, FormatJSONC, FormatTOML)
	}
	return newValueError("content must be FileContent, TreeContent, SectionContent or StructuredContent")
}

func modelContent(content Content) Value {
	switch c := content.(type) {
	case FileContent:
		return c.modelValue()
	case TreeContent:
		return c.modelValue()
	case SectionContent:
		return c.modelValue()
	case StructuredContent:
		return c.modelValue()
	}
	return Null{}
}

// FileContent is a whole file's bytes and mode.
type FileContent struct {
	data []byte
	mode FileMode
}

// NewFileContent returns file content holding a copy of data.
func NewFileContent(data []byte, mode FileMode) (FileContent, error) {
	if err := checkEnum(mode, "FileMode", ModePrivate, ModeReadable, ModeExecutable); err != nil {
		return FileContent{}, err
	}
	return FileContent{data: slices.Clone(data), mode: mode}, nil
}

// Data returns a copy of the file bytes.
func (c FileContent) Data() []byte { return append([]byte{}, c.data...) }

// Mode returns the file mode.
func (c FileContent) Mode() FileMode { return c.mode }

func (c FileContent) modelValue() Value {
	return typedRecord("FileContent", field("data", modelBytes(c.data)), field("mode", modelText(string(c.mode))))
}

// TreeContent is a directory tree snapshot; the zero value is an empty tree.
type TreeContent struct {
	entries []BundleEntry
}

// NewTreeContent returns a tree of copies of entries with missing parent directories inferred, in path byte order.
func NewTreeContent(entries []BundleEntry) (TreeContent, error) {
	for _, entry := range entries {
		if err := validateEntry(entry); err != nil {
			return TreeContent{}, err
		}
	}
	snapshot, err := snapshotEntries(entries)
	if err != nil {
		return TreeContent{}, err
	}
	return TreeContent{entries: snapshot}, nil
}

// Entries returns copies of the tree entries.
func (c TreeContent) Entries() []BundleEntry { return copyEntries(c.entries) }

func (c TreeContent) modelValue() Value {
	return typedRecord("TreeContent", field("entries", modelList(c.entries, modelEntry)))
}

// SelectorPart is one step of a Selector: a Key or a Member.
type SelectorPart interface {
	isSelectorPart()
}

func (Key) isSelectorPart()    {}
func (Member) isSelectorPart() {}

// Key selects an object member by name.
type Key struct {
	name string
}

// NewKey returns a key selecting the member named name.
func NewKey(name string) (Key, error) {
	if name == "" {
		return Key{}, newValueError("selector keys must be nonempty")
	}
	if err := validText(name, "selector key"); err != nil {
		return Key{}, err
	}
	return Key{name: name}, nil
}

// Name returns the member name.
func (k Key) Name() string { return k.name }

func (k Key) modelValue() Value {
	return typedRecord("Key", field("name", modelText(k.name)))
}

// Member selects an array member by its identity, or by the identity of the fields named by its key.
type Member struct {
	identity Value
	key      []string
}

// NewMember returns a member selector; identity must be a scalar when key names fields.
func NewMember(identity Value, key []string) (Member, error) {
	if _, err := checkValue(identity); err != nil {
		return Member{}, err
	}
	for _, name := range key {
		if _, err := NewKey(name); err != nil {
			return Member{}, err
		}
	}
	if _, scalar := identity.(Scalar); len(key) > 0 && !scalar {
		return Member{}, newValueError("native member identity must be a scalar")
	}
	return Member{identity: identity, key: slices.Clone(key)}, nil
}

// Identity returns the member identity.
func (m Member) Identity() Value { return m.identity }

// Key returns the names of the fields that hold the identity.
func (m Member) Key() []string { return slices.Clone(m.key) }

func (m Member) valid() error {
	_, err := checkValue(m.identity)
	return err
}

func (m Member) equal(o Member) bool {
	return ValuesEqual(m.identity, o.identity) && slices.Equal(m.key, o.key)
}

func (m Member) modelValue() Value {
	return typedRecord("Member",
		field("identity", modelOfValue(m.identity)),
		field("key", modelList(m.key, modelText)),
	)
}

func checkSelectorPart(part SelectorPart) error {
	switch p := part.(type) {
	case Key:
		if p.name == "" {
			return newValueError("selector keys must be nonempty")
		}
		return nil
	case Member:
		return p.valid()
	}
	return newValueError("selector parts must be Key or Member values")
}

func modelSelectorPart(part SelectorPart) Value {
	switch p := part.(type) {
	case Key:
		return p.modelValue()
	case Member:
		return p.modelValue()
	}
	return Null{}
}

// Selector is a nonempty path of keys and members into a structured document.
type Selector struct {
	parts []SelectorPart
}

// NewSelector returns a selector of parts in order.
func NewSelector(parts ...SelectorPart) (Selector, error) {
	if len(parts) == 0 {
		return Selector{}, newValueError("a selector must select a key or member")
	}
	for _, part := range parts {
		if err := checkSelectorPart(part); err != nil {
			return Selector{}, err
		}
	}
	return Selector{parts: slices.Clone(parts)}, nil
}

// Parts returns a copy of the selector parts.
func (s Selector) Parts() []SelectorPart { return slices.Clone(s.parts) }

func (s Selector) modelValue() Value {
	return typedRecord("Selector", field("parts", modelList(s.parts, modelSelectorPart)))
}

// Placement is where a new array member is inserted; the zero value is Rust's default, last with no anchor.
type Placement struct {
	position Position
	anchor   *Member
}

// NewPlacement returns a placement; before and after require an anchor, first and last forbid one.
func NewPlacement(position Position, anchor *Member) (Placement, error) {
	if err := checkEnum(position, "Position", PositionFirst, PositionLast, PositionBefore, PositionAfter); err != nil {
		return Placement{}, err
	}
	anchored := position == PositionBefore || position == PositionAfter
	if anchored != (anchor != nil) {
		return Placement{}, newValueError("before/after placement requires exactly one member anchor")
	}
	if anchor == nil {
		return Placement{position: position}, nil
	}
	if err := anchor.valid(); err != nil {
		return Placement{}, err
	}
	copied := *anchor
	return Placement{position: position, anchor: &copied}, nil
}

// Position returns the insertion position.
func (p Placement) Position() Position {
	if p.position == "" {
		return PositionLast
	}
	return p.position
}

// Anchor returns the anchor member, if any.
func (p Placement) Anchor() (Member, bool) {
	if p.anchor == nil {
		return Member{}, false
	}
	return *p.anchor, true
}

func (p Placement) modelValue() Value {
	anchor := Value(Null{})
	if p.anchor != nil {
		anchor = p.anchor.modelValue()
	}
	return typedRecord("Placement", field("anchor", anchor), field("position", modelText(string(p.Position()))))
}

// SectionBoundaries are the distinct start and end lines delimiting a managed text section.
type SectionBoundaries struct {
	start string
	end   string
}

// NewSectionBoundaries returns validated section boundaries.
func NewSectionBoundaries(start, end string) (SectionBoundaries, error) {
	for _, line := range []string{start, end} {
		if err := validText(line, "section boundary"); err != nil {
			return SectionBoundaries{}, err
		}
		if unicodex.IsPythonBlank(line) || strings.ContainsAny(line, "\r\n\x00") {
			return SectionBoundaries{}, newValueError("section boundaries must be nonblank single lines without NUL")
		}
	}
	if start == end {
		return SectionBoundaries{}, newValueError("section boundaries must be distinct")
	}
	return SectionBoundaries{start: start, end: end}, nil
}

// SectionBoundariesFromMarker returns the default Flyrail comment boundaries for marker.
func SectionBoundariesFromMarker(marker string) (SectionBoundaries, error) {
	if err := validateIdentifier(marker, "section marker"); err != nil {
		return SectionBoundaries{}, err
	}
	return NewSectionBoundaries("<!-- flyrail:"+marker+":start -->", "<!-- flyrail:"+marker+":end -->")
}

// Start returns the start line.
func (b SectionBoundaries) Start() string { return b.start }

// End returns the end line.
func (b SectionBoundaries) End() string { return b.end }

func (b SectionBoundaries) modelValue() Value {
	return typedRecord("SectionBoundaries", field("end", modelText(b.end)), field("start", modelText(b.start)))
}

// SectionContent is managed text inside a document section.
type SectionContent struct {
	marker     string
	text       string
	boundaries *SectionBoundaries
}

// NewSectionContent returns section content; nil boundaries derive from the marker.
func NewSectionContent(marker, text string, boundaries *SectionBoundaries) (SectionContent, error) {
	if err := validateIdentifier(marker, "section marker"); err != nil {
		return SectionContent{}, err
	}
	if err := validText(text, "section text"); err != nil {
		return SectionContent{}, err
	}
	if strings.Contains(text, "\x00") || strings.Contains(text, "<!-- flyrail:") {
		return SectionContent{}, newValueError("section text cannot contain NUL or Flyrail markers")
	}
	if boundaries == nil {
		return SectionContent{marker: marker, text: text}, nil
	}
	if boundaries.start == "" {
		return SectionContent{}, newValueError("section boundaries must be nonblank single lines without NUL")
	}
	if slices.ContainsFunc(pythonLines(text), func(line string) bool {
		return line == boundaries.start || line == boundaries.end
	}) {
		return SectionContent{}, newValueError("section text cannot contain its boundary lines")
	}
	copied := *boundaries
	return SectionContent{marker: marker, text: text, boundaries: &copied}, nil
}

// Marker returns the section marker.
func (c SectionContent) Marker() string { return c.marker }

// Text returns the section text.
func (c SectionContent) Text() string { return c.text }

// Boundaries returns the custom boundaries, if any.
func (c SectionContent) Boundaries() (SectionBoundaries, bool) {
	if c.boundaries == nil {
		return SectionBoundaries{}, false
	}
	return *c.boundaries, true
}

// Selector returns the custom boundaries or those derived from the marker.
func (c SectionContent) Selector() SectionBoundaries {
	if c.boundaries != nil {
		return *c.boundaries
	}
	boundaries, _ := SectionBoundariesFromMarker(c.marker)
	return boundaries
}

func (c SectionContent) modelValue() Value {
	boundaries := Value(Null{})
	if c.boundaries != nil {
		boundaries = c.boundaries.modelValue()
	}
	return typedRecord("SectionContent",
		field("boundaries", boundaries),
		field("marker", modelText(c.marker)),
		field("text", modelText(c.text)),
	)
}

// StructuredContent is a value set at a selector inside a structured document.
type StructuredContent struct {
	format    DocumentFormat
	selector  Selector
	value     Value
	placement *Placement
}

// NewStructuredContent returns structured content; a placement is only valid when the selector ends in a member.
func NewStructuredContent(format DocumentFormat, selector Selector, value Value, placement *Placement) (StructuredContent, error) {
	if err := checkEnum(format, "DocumentFormat", FormatJSON, FormatJSONC, FormatTOML); err != nil {
		return StructuredContent{}, err
	}
	if len(selector.parts) == 0 {
		return StructuredContent{}, newValueError("a selector must select a key or member")
	}
	if _, err := checkValue(value); err != nil {
		return StructuredContent{}, err
	}
	if placement == nil {
		return StructuredContent{format: format, selector: selector, value: value}, nil
	}
	member, ok := selector.parts[len(selector.parts)-1].(Member)
	if !ok {
		return StructuredContent{}, newValueError("placement is only valid for an array member")
	}
	if anchor, ok := placement.Anchor(); ok && anchor.equal(member) {
		return StructuredContent{}, newValueError("a member cannot be its own insertion anchor")
	}
	copied := *placement
	return StructuredContent{format: format, selector: selector, value: value, placement: &copied}, nil
}

// Format returns the document format.
func (c StructuredContent) Format() DocumentFormat { return c.format }

// Selector returns the selector.
func (c StructuredContent) Selector() Selector { return c.selector }

// Value returns the value set at the selector.
func (c StructuredContent) Value() Value { return c.value }

// Placement returns the explicit placement, if any; an absent placement differs from the default one.
func (c StructuredContent) Placement() (Placement, bool) {
	if c.placement == nil {
		return Placement{}, false
	}
	return *c.placement, true
}

func (c StructuredContent) modelValue() Value {
	placement := Value(Null{})
	if c.placement != nil {
		placement = c.placement.modelValue()
	}
	return typedRecord("StructuredContent",
		field("format", modelText(string(c.format))),
		field("placement", placement),
		field("selector", c.selector.modelValue()),
		field("value", modelOfValue(c.value)),
	)
}

// DocumentSchema is a top-level scalar a JSON or JSONC document must carry.
type DocumentSchema struct {
	format DocumentFormat
	key    string
	value  Scalar
}

// NewDocumentSchema returns a schema requirement for key in a JSON or JSONC document.
func NewDocumentSchema(format DocumentFormat, key string, value Scalar) (DocumentSchema, error) {
	if format != FormatJSON && format != FormatJSONC {
		return DocumentSchema{}, newValueError("document schema scaffolds require JSON or JSONC")
	}
	if _, err := NewKey(key); err != nil {
		return DocumentSchema{}, err
	}
	if _, err := checkValue(value); err != nil {
		return DocumentSchema{}, err
	}
	return DocumentSchema{format: format, key: key, value: value}, nil
}

// Format returns the document format.
func (s DocumentSchema) Format() DocumentFormat { return s.format }

// Key returns the top-level key.
func (s DocumentSchema) Key() string { return s.key }

// Value returns the required scalar.
func (s DocumentSchema) Value() Scalar { return s.value }

// Content returns the structured content that sets the schema key.
func (s DocumentSchema) Content() (StructuredContent, error) {
	key, err := NewKey(s.key)
	if err != nil {
		return StructuredContent{}, err
	}
	selector, err := NewSelector(key)
	if err != nil {
		return StructuredContent{}, err
	}
	return NewStructuredContent(s.format, selector, s.value, nil)
}

func (s DocumentSchema) modelValue() Value {
	return typedRecord("DocumentSchema",
		field("format", modelText(string(s.format))),
		field("key", modelText(s.key)),
		field("value", modelOfValue(s.value)),
	)
}

func isPythonLineBreak(r rune) bool {
	switch r {
	case '\n', '\r', '\v', '\f', 0x1c, 0x1d, 0x1e, 0x85, 0x2028, 0x2029:
		return true
	}
	return false
}

func pythonLines(text string) []string {
	var lines []string
	rest := text
	for rest != "" {
		index := strings.IndexFunc(rest, isPythonLineBreak)
		if index < 0 {
			return append(lines, rest)
		}
		r, width := utf8.DecodeRuneInString(rest[index:])
		if r == '\r' && strings.HasPrefix(rest[index+1:], "\n") {
			width = 2
		}
		lines = append(lines, rest[:index])
		rest = rest[index+width:]
	}
	return lines
}
