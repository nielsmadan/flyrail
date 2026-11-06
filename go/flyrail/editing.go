package flyrail

import (
	"bytes"
	"errors"
	"fmt"
	"slices"
	"strings"
	"unicode/utf8"
)

// Acquisition is how a request may acquire a selection that already exists in a document.
type Acquisition string

// The acquisition modes, spelled as on the wire.
const (
	AcquireConflict Acquisition = "conflict"
	AcquireTakeover Acquisition = "takeover"
	AcquireAdopt    Acquisition = "adopt"
)

// ParseAcquisition returns the Acquisition spelled s.
func ParseAcquisition(s string) (Acquisition, error) {
	return parseEnum(s, "Acquisition", AcquireConflict, AcquireTakeover, AcquireAdopt)
}

// EditStatus is the outcome of one document request.
type EditStatus string

// The edit statuses, spelled as on the wire.
const (
	EditCreated   EditStatus = "created"
	EditTakenOver EditStatus = "taken-over"
	EditAdopted   EditStatus = "adopted"
	EditCurrent   EditStatus = "current"
	EditUpdated   EditStatus = "updated"
	EditRemoved   EditStatus = "removed"
	EditModified  EditStatus = "modified"
	EditConflict  EditStatus = "conflict"
)

func blockingStatus(status EditStatus) bool {
	return status == EditConflict || status == EditModified
}

// DocumentRequest asks the editor to set or retire one selection of a shared document.
type DocumentRequest struct {
	claim       Claim
	content     Content
	acquisition Acquisition
}

// NewDocumentRequest returns a request for claim; nil content retires it and an empty acquisition means AcquireConflict.
func NewDocumentRequest(claim Claim, content Content, acquisition Acquisition) (DocumentRequest, error) {
	if acquisition == "" {
		acquisition = AcquireConflict
	}
	if err := checkEnum(acquisition, "Acquisition", AcquireConflict, AcquireTakeover, AcquireAdopt); err != nil {
		return DocumentRequest{}, err
	}
	if claim.selector == nil || claim.kind != OwnershipSection && claim.kind != OwnershipStructured {
		return DocumentRequest{}, newValueError("document requests require section or structured claims")
	}
	if content != nil {
		expected, err := ContentClaim(content)
		if err != nil {
			return DocumentRequest{}, err
		}
		if !claim.Equal(expected) {
			return DocumentRequest{}, newValueError("requested content must address its physical claim")
		}
	} else if acquisition != AcquireConflict {
		return DocumentRequest{}, newValueError("retirement does not acquire ownership")
	}
	return DocumentRequest{claim: claim, content: content, acquisition: acquisition}, nil
}

// NewSetRequest returns a request that sets content at the claim it addresses.
func NewSetRequest(content Content, acquisition Acquisition) (DocumentRequest, error) {
	claim, err := ContentClaim(content)
	if err != nil {
		return DocumentRequest{}, err
	}
	return NewDocumentRequest(claim, content, acquisition)
}

// NewRetireRequest returns a request that removes an owned claim.
func NewRetireRequest(claim Claim) (DocumentRequest, error) {
	return NewDocumentRequest(claim, nil, AcquireConflict)
}

// Claim returns the requested claim.
func (r DocumentRequest) Claim() Claim { return r.claim }

// Content returns the requested content, or nil for a retirement.
func (r DocumentRequest) Content() Content { return r.content }

// Acquisition returns how the request may acquire an existing selection.
func (r DocumentRequest) Acquisition() Acquisition { return r.acquisition }

// EditObservation reports what the editor found and decided for one claim.
type EditObservation struct {
	Claim   Claim
	Status  EditStatus
	Current *SelectionSnapshot
	Message string
}

// DocumentEdit is the result of editing one shared document.
type DocumentEdit struct {
	Before       []byte
	BeforeExists bool
	After        []byte
	AfterExists  bool
	Ownership    []OwnedSelection
	Provenance   DocumentProvenance
	Observations []EditObservation
}

// Applicable reports whether no request ended in a conflict or modification.
func (e DocumentEdit) Applicable() bool {
	return !slices.ContainsFunc(e.Observations, func(item EditObservation) bool { return blockingStatus(item.Status) })
}

type document struct {
	data   []byte
	exists bool
}

func (d document) clone() document {
	if !d.exists {
		return document{}
	}
	return document{data: cloneBytes(d.data), exists: true}
}

func newDocumentEdit(before, after document, ownership []OwnedSelection, provenance DocumentProvenance, observations []EditObservation) DocumentEdit {
	before, after = before.clone(), after.clone()
	return DocumentEdit{
		Before:       before.data,
		BeforeExists: before.exists,
		After:        after.data,
		AfterExists:  after.exists,
		Ownership:    slices.Clone(ownership),
		Provenance:   provenance,
		Observations: observations,
	}
}

// EditDocument applies requests to a shared document whose current bytes are source (absent unless sourceExists).
func EditDocument(source []byte, sourceExists bool, requests []DocumentRequest, ownership []OwnedSelection, provenance DocumentProvenance) (DocumentEdit, error) {
	if err := checkEditInputs(requests, ownership); err != nil {
		return DocumentEdit{}, err
	}
	data := document{data: source, exists: sourceExists}.clone()
	format, err := validateInputs(requests, ownership)
	if err != nil {
		return DocumentEdit{}, err
	}
	text, hasBOM, err := decodeDocument(data.data)
	if err != nil {
		return DocumentEdit{}, err
	}
	if len(requests) == 0 {
		return newDocumentEdit(data, data, ownership, provenance, nil), nil
	}
	if format == "" {
		return editSections(data, text, hasBOM, requests, ownership, provenance)
	}
	return editStructured(data, text, hasBOM, format, requests, ownership, provenance)
}

func checkEditInputs(requests []DocumentRequest, ownership []OwnedSelection) error {
	for _, request := range requests {
		if request.claim.selector == nil {
			return newValueError("document requests require section or structured claims")
		}
	}
	for _, item := range ownership {
		if item.claim.selector == nil || item.installed.value == nil {
			return newTypeError("ownership requires a Claim")
		}
	}
	return nil
}

func observeSelections(data document, ownership []OwnedSelection) ([]EditObservation, error) {
	if err := checkEditInputs(nil, ownership); err != nil {
		return nil, err
	}
	format, err := validateInputs(nil, ownership)
	if err != nil {
		return nil, err
	}
	text, _, err := decodeDocument(data.data)
	if err != nil {
		return nil, err
	}
	snapshots := make([]*SelectionSnapshot, 0, len(ownership))
	if format == "" {
		var boundaries []SectionBoundaries
		for _, item := range ownership {
			if pair, ok := item.claim.selector.(SectionBoundaries); ok {
				boundaries = append(boundaries, pair)
			}
		}
		spans, err := sectionSpans(text, boundaries)
		if err != nil {
			return nil, err
		}
		for _, item := range ownership {
			var current *SelectionSnapshot
			if pair, ok := item.claim.selector.(SectionBoundaries); ok {
				current = sectionSnapshot(text, findSpan(spans, pair))
			}
			snapshots = append(snapshots, current)
		}
	} else {
		backend, err := parseStructured(text, format)
		if err != nil {
			return nil, err
		}
		for _, item := range ownership {
			var current *SelectionSnapshot
			if selector, ok := item.claim.selector.(Selector); ok {
				if current, err = structuredSnapshot(backend, selector); err != nil {
					return nil, err
				}
			}
			snapshots = append(snapshots, current)
		}
	}
	observations := make([]EditObservation, 0, len(ownership))
	for index, item := range ownership {
		current := snapshots[index]
		want := desiredOfSnapshot(item.installed.value)
		status := selectionStatus(current, want, &item, AcquireConflict, format)
		observations = append(observations, EditObservation{Claim: item.claim, Status: status, Current: current})
	}
	return observations, nil
}

func validateInputs(requests []DocumentRequest, ownership []OwnedSelection) (DocumentFormat, error) {
	for index, item := range requests {
		if slices.ContainsFunc(requests[:index], func(other DocumentRequest) bool { return other.claim.Equal(item.claim) }) {
			return "", newValueError("duplicate document claim")
		}
	}
	for index, item := range ownership {
		if slices.ContainsFunc(ownership[:index], func(other OwnedSelection) bool { return other.claim.Equal(item.claim) }) {
			return "", newValueError("duplicate document claim")
		}
	}
	var claims []Claim
	addClaim := func(claim Claim) {
		if !slices.ContainsFunc(claims, claim.Equal) {
			claims = append(claims, claim)
		}
	}
	for _, request := range requests {
		addClaim(request.claim)
	}
	for _, item := range ownership {
		addClaim(item.claim)
	}
	for index, claim := range claims {
		if slices.ContainsFunc(claims[index+1:], claim.Overlaps) {
			return "", newValueError("document claims overlap")
		}
	}
	formats := make([]DocumentFormat, 0, len(ownership)+len(requests))
	for _, item := range ownership {
		formats = append(formats, item.format)
	}
	for _, request := range requests {
		if request.content == nil {
			continue
		}
		if structured, ok := request.content.(StructuredContent); ok {
			formats = append(formats, structured.format)
		} else {
			formats = append(formats, "")
		}
	}
	var distinct []DocumentFormat
	for _, format := range formats {
		if !slices.Contains(distinct, format) {
			distinct = append(distinct, format)
		}
	}
	if len(distinct) > 1 {
		return "", newValueError("one document must use one editor format")
	}
	if len(distinct) == 0 {
		return "", nil
	}
	return distinct[0], nil
}

func decodeUTF8(data []byte) (string, error) {
	start := 0
	for start < len(data) {
		r, size := utf8.DecodeRune(data[start:])
		if r == utf8.RuneError && size <= 1 {
			break
		}
		start += size
	}
	if start == len(data) {
		return string(data), nil
	}
	length, truncated := invalidUTF8Length(data[start:])
	end := start + length
	description, position := "bytes", fmt.Sprintf("position %d-%d", start, end-1)
	if length == 1 {
		description, position = fmt.Sprintf("byte 0x%02x", data[start]), fmt.Sprintf("position %d", start)
	}
	reason := "invalid continuation byte"
	switch {
	case truncated:
		reason = "unexpected end of data"
	case data[start] < 0xC2 || data[start] > 0xF4:
		reason = "invalid start byte"
	}
	return "", newValueError(fmt.Sprintf("'utf-8' codec can't decode %s in %s: %s", description, position, reason))
}

// Reproduces Rust's error_len: the maximal invalid subpart.
func invalidUTF8Length(data []byte) (int, bool) {
	lead := data[0]
	var width int
	switch {
	case lead >= 0xC2 && lead <= 0xDF:
		width = 2
	case lead >= 0xE0 && lead <= 0xEF:
		width = 3
	case lead >= 0xF0 && lead <= 0xF4:
		width = 4
	default:
		return 1, false
	}
	low, high := byte(0x80), byte(0xBF)
	switch lead {
	case 0xE0:
		low = 0xA0
	case 0xED:
		high = 0x9F
	case 0xF0:
		low = 0x90
	case 0xF4:
		high = 0x8F
	}
	for index := 1; index < width; index++ {
		if index >= len(data) {
			return len(data), true
		}
		if data[index] < low || data[index] > high {
			return index, false
		}
		low, high = 0x80, 0xBF
	}
	return width, false
}

const utf8BOM = "\xef\xbb\xbf"

func decodeDocument(data []byte) (string, bool, error) {
	body, hasBOM := bytes.CutPrefix(data, []byte(utf8BOM))
	text, err := decodeUTF8(body)
	if err != nil {
		return "", false, err
	}
	if strings.Contains(text, "\x00") {
		return "", false, newValueError("documents cannot contain NUL")
	}
	return text, hasBOM, nil
}

func withBOM(hasBOM bool, text string) []byte {
	after := make([]byte, 0, len(text)+len(utf8BOM))
	if hasBOM {
		after = append(after, utf8BOM...)
	}
	return append(after, text...)
}

type desiredKind int

const (
	desiredRetired desiredKind = iota
	desiredSection
	desiredStructured
)

type desired struct {
	kind    desiredKind
	section []byte
	value   Value
}

func desiredOfSnapshot(value SnapshotValue) desired {
	switch v := value.(type) {
	case SnapshotBytes:
		return desired{kind: desiredSection, section: v.data}
	case SnapshotTyped:
		return desired{kind: desiredStructured, value: v.value}
	}
	return desired{kind: desiredRetired}
}

func desiredMatches(value SnapshotValue, want desired) bool {
	switch v := value.(type) {
	case SnapshotBytes:
		return want.kind == desiredSection && bytes.Equal(v.data, want.section)
	case SnapshotTyped:
		return want.kind == desiredStructured && ValuesEqual(v.value, want.value)
	}
	return false
}

func selectionStatus(current *SelectionSnapshot, want desired, owned *OwnedSelection, acquisition Acquisition, format DocumentFormat) EditStatus {
	if owned != nil {
		installed := owned.installed
		if current == nil {
			return EditModified
		}
		if !snapshotValuesEqual(current.value, installed.value) ||
			format != FormatJSON && !bytes.Equal(current.ownedSyntax, installed.ownedSyntax) {
			return EditModified
		}
		if want.kind == desiredRetired {
			return EditRemoved
		}
		if desiredMatches(current.value, want) {
			return EditCurrent
		}
		return EditUpdated
	}
	if want.kind == desiredRetired {
		return EditConflict
	}
	if current == nil {
		return EditCreated
	}
	if acquisition == AcquireTakeover {
		return EditTakenOver
	}
	if acquisition == AcquireAdopt && desiredMatches(current.value, want) {
		return EditAdopted
	}
	return EditConflict
}

func dispositionFromStatus(status EditStatus) (Disposition, error) {
	switch status {
	case EditCreated:
		return DispositionCreated, nil
	case EditTakenOver:
		return DispositionTakenOver, nil
	case EditAdopted:
		return DispositionAdopted, nil
	}
	return "", newValueError("'" + string(status) + "' is not a valid Disposition")
}

func ownershipFor(claim Claim, format DocumentFormat, installed SelectionSnapshot, current *SelectionSnapshot, previous *OwnedSelection, status EditStatus, separator []byte) (OwnedSelection, error) {
	if previous != nil {
		return NewOwnedSelection(OwnedSelectionParams{
			Claim:       previous.claim,
			Format:      previous.format,
			Installed:   installed,
			Disposition: previous.disposition,
			Baseline:    previous.baseline,
			Separator:   previous.separator,
		})
	}
	disposition, err := dispositionFromStatus(status)
	if err != nil {
		return OwnedSelection{}, err
	}
	var baseline *SelectionSnapshot
	if disposition == DispositionTakenOver {
		baseline = current
	}
	return NewOwnedSelection(OwnedSelectionParams{
		Claim:       claim,
		Format:      format,
		Installed:   installed,
		Disposition: disposition,
		Baseline:    baseline,
		Separator:   separator,
	})
}

func findOwned(owned []OwnedSelection, claim Claim) *OwnedSelection {
	index := slices.IndexFunc(owned, func(item OwnedSelection) bool { return item.claim.Equal(claim) })
	if index < 0 {
		return nil
	}
	found := owned[index]
	return &found
}

func upsertOwned(owned []OwnedSelection, claim Claim, selection OwnedSelection) []OwnedSelection {
	index := slices.IndexFunc(owned, func(item OwnedSelection) bool { return item.claim.Equal(claim) })
	if index < 0 {
		return append(owned, selection)
	}
	owned[index] = selection
	return owned
}

func withoutOwned(owned []OwnedSelection, claim Claim) []OwnedSelection {
	return slices.DeleteFunc(owned, func(item OwnedSelection) bool { return item.claim.Equal(claim) })
}

func withEmptyDocument(provenance DocumentProvenance) DocumentProvenance {
	provenance.emptyDocument = []byte{}
	provenance.hasEmptyDocument = true
	return provenance
}

func editResult(before document, after []byte, previous, owned []OwnedSelection, oldProvenance, provenance DocumentProvenance, observations []EditObservation) DocumentEdit {
	if slices.ContainsFunc(observations, func(item EditObservation) bool { return blockingStatus(item.Status) }) {
		return newDocumentEdit(before, before, previous, oldProvenance, observations)
	}
	if len(owned) > 0 {
		return newDocumentEdit(before, document{data: after, exists: true}, owned, provenance, observations)
	}
	for _, suffix := range []string{"\r\n", "\n", "\r"} {
		body, ok := bytes.CutSuffix(after, []byte(suffix))
		if ok && slices.Contains(provenance.trailingSeparators, sha256Hex(body)) {
			after = body
			break
		}
	}
	final := document{data: after, exists: true}
	if empty, ok := provenance.EmptyDocument(); ok && bytes.Equal(after, empty) {
		final = document{}
	}
	return newDocumentEdit(before, final, owned, DocumentProvenance{}, observations)
}

func asValueError(err error) (*ValueError, bool) {
	return errors.AsType[*ValueError](err)
}
