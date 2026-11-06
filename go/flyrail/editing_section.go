package flyrail

import (
	"cmp"
	"slices"
	"strings"
)

const flyrailMarkerPrefix = "<!-- flyrail:"

type sectionSpan struct {
	boundaries SectionBoundaries
	start, end int
}

type markerLine struct {
	line    string
	pair    SectionBoundaries
	opening bool
}

type sectionEdit struct {
	start, end  int
	replacement string
}

type sectionAddition struct {
	claim  Claim
	text   string
	status EditStatus
}

func detectNewline(text string) string {
	index := strings.IndexAny(text, "\r\n")
	switch {
	case index < 0 || text[index] == '\n':
		return "\n"
	case strings.HasPrefix(text[index:], "\r\n"):
		return "\r\n"
	}
	return "\r"
}

func normalizeNewlines(text, newline string) string {
	var out strings.Builder
	out.Grow(len(text))
	rest := text
	for {
		index := strings.IndexAny(rest, "\r\n")
		if index < 0 {
			break
		}
		out.WriteString(rest[:index])
		out.WriteString(newline)
		width := 1
		if strings.HasPrefix(rest[index:], "\r\n") {
			width = 2
		}
		rest = rest[index+width:]
	}
	out.WriteString(rest)
	return out.String()
}

func generatedSection(content SectionContent, newline string) string {
	selector := content.Selector()
	body := normalizeNewlines(content.text, newline)
	if body != "" && !strings.HasSuffix(body, newline) {
		body += newline
	}
	return selector.start + newline + body + selector.end + newline
}

func sectionLines(text string) ([]int, []string) {
	var offsets []int
	var lines []string
	offset := 0
	for offset < len(text) {
		rest := text[offset:]
		consumed := len(rest)
		if index := strings.IndexAny(rest, "\r\n"); index >= 0 {
			consumed = index + 1
			if strings.HasPrefix(rest[index:], "\r\n") {
				consumed = index + 2
			}
		}
		offsets = append(offsets, offset)
		lines = append(lines, rest[:consumed])
		offset += consumed
	}
	return offsets, lines
}

func stripLineNewline(line string) string {
	for _, suffix := range []string{"\r\n", "\n", "\r"} {
		if bare, ok := strings.CutSuffix(line, suffix); ok {
			return bare
		}
	}
	return line
}

func scanMarkers(text string, pairs []SectionBoundaries) ([]SectionBoundaries, error) {
	cursor := 0
	for {
		offset := strings.Index(text[cursor:], flyrailMarkerPrefix)
		if offset < 0 {
			return pairs, nil
		}
		markerStart := cursor + offset + len(flyrailMarkerPrefix)
		regionEnd := len(text)
		if position := strings.IndexAny(text[markerStart:], "\r\n<>"); position >= 0 {
			regionEnd = markerStart + position
		}
		matched := false
		for colon := regionEnd - 1; colon >= markerStart && !matched; colon-- {
			if text[colon] != ':' || colon == markerStart {
				continue
			}
			after := text[colon+1:]
			for _, keyword := range []string{"start", "end"} {
				rest, ok := strings.CutPrefix(after, keyword)
				if !ok || !strings.HasPrefix(rest, " -->") {
					continue
				}
				pair, err := SectionBoundariesFromMarker(text[markerStart:colon])
				if err != nil {
					return nil, err
				}
				if !slices.Contains(pairs, pair) {
					pairs = append(pairs, pair)
				}
				matched = true
				break
			}
		}
		cursor = markerStart
	}
}

func sectionSpans(text string, boundaries []SectionBoundaries) ([]sectionSpan, error) {
	var pairs []SectionBoundaries
	for _, boundary := range boundaries {
		if !slices.Contains(pairs, boundary) {
			pairs = append(pairs, boundary)
		}
	}
	pairs, err := scanMarkers(text, pairs)
	if err != nil {
		return nil, err
	}
	var markers []markerLine
	for _, pair := range pairs {
		for _, candidate := range []markerLine{{line: pair.start, pair: pair, opening: true}, {line: pair.end, pair: pair}} {
			if slices.ContainsFunc(markers, func(existing markerLine) bool { return existing.line == candidate.line }) {
				return nil, newValueError("section boundaries overlap")
			}
			markers = append(markers, candidate)
		}
	}
	var spans []sectionSpan
	var opened *sectionSpan
	offsets, lines := sectionLines(text)
	for index, line := range lines {
		bare := stripLineNewline(line)
		found := slices.IndexFunc(markers, func(existing markerLine) bool { return existing.line == bare })
		if found < 0 {
			if strings.Contains(bare, flyrailMarkerPrefix) ||
				slices.ContainsFunc(markers, func(existing markerLine) bool { return strings.Contains(bare, existing.line) }) {
				return nil, newValueError("section boundaries must occupy complete lines")
			}
			continue
		}
		marker := markers[found]
		if marker.opening {
			if opened != nil || slices.ContainsFunc(spans, func(span sectionSpan) bool { return span.boundaries == marker.pair }) {
				return nil, newValueError("nested or repeated section boundaries")
			}
			opened = &sectionSpan{boundaries: marker.pair, start: offsets[index]}
			continue
		}
		if opened == nil || opened.boundaries != marker.pair {
			return nil, newValueError("malformed or intersecting section boundaries")
		}
		spans = append(spans, sectionSpan{boundaries: marker.pair, start: opened.start, end: offsets[index] + len(line)})
		opened = nil
	}
	if opened != nil {
		return nil, newValueError("section has no closing boundary")
	}
	return spans, nil
}

func findSpan(spans []sectionSpan, pair SectionBoundaries) *sectionSpan {
	index := slices.IndexFunc(spans, func(span sectionSpan) bool { return span.boundaries == pair })
	if index < 0 {
		return nil
	}
	return &spans[index]
}

func sectionSnapshotOf(text string) SelectionSnapshot {
	raw := []byte(text)
	return SelectionSnapshot{value: SnapshotBytes{data: raw}, syntax: raw, ownedSyntax: raw}
}

func sectionSnapshot(text string, span *sectionSpan) *SelectionSnapshot {
	if span == nil {
		return nil
	}
	snapshot := sectionSnapshotOf(text[span.start:span.end])
	return &snapshot
}

func editSections(data document, text string, hasBOM bool, requests []DocumentRequest, previous []OwnedSelection, provenance DocumentProvenance) (DocumentEdit, error) {
	var boundaries []SectionBoundaries
	for _, claim := range requestAndOwnedClaims(requests, previous) {
		pair, ok := claim.selector.(SectionBoundaries)
		if !ok {
			return DocumentEdit{}, newValueError("retirement requires existing ownership and its document format")
		}
		boundaries = append(boundaries, pair)
	}
	spans, err := sectionSpans(text, boundaries)
	if err != nil {
		return DocumentEdit{}, err
	}
	owned := slices.Clone(previous)
	var observations []EditObservation
	var edits []sectionEdit
	var additions []sectionAddition
	newline := detectNewline(text)
	for _, request := range requests {
		pair, ok := request.claim.selector.(SectionBoundaries)
		if !ok {
			return DocumentEdit{}, newValueError("text editor requires section boundaries")
		}
		span := findSpan(spans, pair)
		current := sectionSnapshot(text, span)
		sectionNewline := newline
		if span != nil {
			sectionNewline = detectNewline(text[span.start:span.end])
		}
		want := desired{kind: desiredRetired}
		var desiredText string
		if content, ok := request.content.(SectionContent); ok {
			desiredText = generatedSection(content, sectionNewline)
			want = desired{kind: desiredSection, section: []byte(desiredText)}
		}
		old := findOwned(owned, request.claim)
		status := selectionStatus(current, want, old, request.acquisition, "")
		observations = append(observations, EditObservation{Claim: request.claim, Status: status, Current: current})
		switch {
		case blockingStatus(status):
			continue
		case status == EditRemoved:
			if old == nil || span == nil {
				return DocumentEdit{}, newValueError("retirement requires existing ownership")
			}
			replacement := ""
			if old.baseline != nil {
				if replacement, err = decodeUTF8(old.baseline.syntax); err != nil {
					return DocumentEdit{}, err
				}
			}
			start := span.start
			if replacement == "" && len(old.separator) > 0 {
				separator, err := decodeUTF8(old.separator)
				if err != nil {
					return DocumentEdit{}, err
				}
				separatorStart := start - len(separator)
				if separatorStart >= 0 && strings.HasSuffix(text[:start], separator) &&
					!slices.ContainsFunc(spans, func(other sectionSpan) bool { return separatorStart < other.end && other.start < start }) {
					start = separatorStart
				}
			}
			edits = append(edits, sectionEdit{start: start, end: span.end, replacement: replacement})
			owned = withoutOwned(owned, request.claim)
		case want.kind == desiredSection && span == nil:
			additions = append(additions, sectionAddition{claim: request.claim, text: desiredText, status: status})
		case want.kind == desiredSection:
			installed := current
			if status == EditUpdated || status == EditTakenOver {
				edits = append(edits, sectionEdit{start: span.start, end: span.end, replacement: desiredText})
				generated := sectionSnapshotOf(desiredText)
				installed = &generated
			}
			if installed == nil {
				return DocumentEdit{}, newValueError("selection is missing")
			}
			selection, err := ownershipFor(request.claim, "", *installed, current, old, status, nil)
			if err != nil {
				return DocumentEdit{}, err
			}
			owned = upsertOwned(owned, request.claim, selection)
		}
	}
	slices.SortFunc(edits, func(a, b sectionEdit) int {
		return cmp.Or(cmp.Compare(a.start, b.start), cmp.Compare(a.end, b.end), strings.Compare(a.replacement, b.replacement))
	})
	for index := 1; index < len(edits); index++ {
		if edits[index-1].end > edits[index].start {
			return DocumentEdit{}, newValueError("section edits overlap")
		}
	}
	body := text
	for _, edit := range slices.Backward(edits) {
		body = body[:edit.start] + edit.replacement + body[edit.end:]
	}
	doubled := newline + newline
	for _, addition := range additions {
		separator := doubled
		switch {
		case body == "" || strings.HasSuffix(body, doubled):
			separator = ""
		case strings.HasSuffix(body, newline):
			separator = newline
		}
		body += separator + addition.text
		selection, err := ownershipFor(addition.claim, "", sectionSnapshotOf(addition.text), nil, nil, addition.status, []byte(separator))
		if err != nil {
			return DocumentEdit{}, err
		}
		owned = upsertOwned(owned, addition.claim, selection)
	}
	if _, err := sectionSpans(body, boundaries); err != nil {
		return DocumentEdit{}, err
	}
	creation := provenance
	if !data.exists && len(owned) > 0 {
		creation = withEmptyDocument(provenance)
	}
	return editResult(data, withBOM(hasBOM, body), previous, owned, provenance, creation, observations), nil
}

func requestAndOwnedClaims(requests []DocumentRequest, owned []OwnedSelection) []Claim {
	claims := make([]Claim, 0, len(requests)+len(owned))
	for _, request := range requests {
		claims = append(claims, request.claim)
	}
	for _, item := range owned {
		claims = append(claims, item.claim)
	}
	return claims
}
