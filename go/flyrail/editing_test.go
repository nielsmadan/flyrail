package flyrail

import (
	"bytes"
	"errors"
	"slices"
	"strings"
	"testing"

	"github.com/nielsmadan/flyrail/go/internal/jsonx"
)

var editFixtureFormats = map[string]bool{"text": true, "json": true, "jsonc": true, "toml": true}

var newlines = []string{"\n", "\r\n", "\r"}

func mustSection(t *testing.T, marker, text string, boundaries *SectionBoundaries) SectionContent {
	t.Helper()
	content, err := NewSectionContent(marker, text, boundaries)
	if err != nil {
		t.Fatal(err)
	}
	return content
}

func mustBoundaries(t *testing.T, start, end string) *SectionBoundaries {
	t.Helper()
	boundaries, err := NewSectionBoundaries(start, end)
	if err != nil {
		t.Fatal(err)
	}
	return &boundaries
}

func mustSetRequest(t *testing.T, content Content, acquisition Acquisition) DocumentRequest {
	t.Helper()
	request, err := NewSetRequest(content, acquisition)
	if err != nil {
		t.Fatal(err)
	}
	return request
}

func mustRetireRequest(t *testing.T, claim Claim) DocumentRequest {
	t.Helper()
	request, err := NewRetireRequest(claim)
	if err != nil {
		t.Fatal(err)
	}
	return request
}

func mustEdit(t *testing.T, data []byte, exists bool, requests []DocumentRequest, ownership []OwnedSelection, provenance DocumentProvenance) DocumentEdit {
	t.Helper()
	edit, err := EditDocument(data, exists, requests, ownership, provenance)
	if err != nil {
		t.Fatal(err)
	}
	return edit
}

func setSection(t *testing.T, data []byte, exists bool, content SectionContent, acquisition Acquisition) DocumentEdit {
	t.Helper()
	return mustEdit(t, data, exists, []DocumentRequest{mustSetRequest(t, content, acquisition)}, nil, DocumentProvenance{})
}

func updateSection(t *testing.T, previous DocumentEdit, content SectionContent, data []byte, exists bool) DocumentEdit {
	t.Helper()
	return mustEdit(t, data, exists, []DocumentRequest{mustSetRequest(t, content, AcquireConflict)}, previous.Ownership, previous.Provenance)
}

func retireAll(t *testing.T, previous DocumentEdit, data []byte, exists bool) DocumentEdit {
	t.Helper()
	requests := make([]DocumentRequest, 0, len(previous.Ownership))
	for _, item := range previous.Ownership {
		requests = append(requests, mustRetireRequest(t, item.Claim()))
	}
	return mustEdit(t, data, exists, requests, previous.Ownership, previous.Provenance)
}

func statuses(edit DocumentEdit) []EditStatus {
	out := make([]EditStatus, 0, len(edit.Observations))
	for _, item := range edit.Observations {
		out = append(out, item.Status)
	}
	return out
}

func ownershipEqual(a, b []OwnedSelection) bool {
	return slices.EqualFunc(a, b, OwnedSelection.equal)
}

func sectionClaim(t *testing.T, content SectionContent) Claim {
	t.Helper()
	claim, err := NewClaim(OwnershipSection, content.Selector())
	if err != nil {
		t.Fatal(err)
	}
	return claim
}

func expectAfter(t *testing.T, label string, edit DocumentEdit, want []byte) {
	t.Helper()
	if !edit.AfterExists || !bytes.Equal(edit.After, want) {
		t.Errorf("%s: after = %q (exists %v), want %q", label, edit.After, edit.AfterExists, want)
	}
}

func editRecordText(record jsonx.Value, name string) (string, bool) {
	item, ok := record.Get(name)
	if !ok {
		return "", false
	}
	return item.AsString()
}

func fixtureBoundaries(selector jsonx.Value) (SectionBoundaries, error) {
	start, _ := editRecordText(selector, "start")
	end, _ := editRecordText(selector, "end")
	return NewSectionBoundaries(start, end)
}

func fixtureMember(record jsonx.Value) (Member, error) {
	var identity Value
	var err error
	if semantic, ok := record.Get("semantic_identity"); ok {
		identity, err = valueFromRecord(semantic)
	} else {
		raw, _ := record.Get("identity")
		identity, err = valueFromJSON(raw)
	}
	if err != nil {
		return Member{}, err
	}
	var key []string
	if names, ok := record.Get("key"); ok {
		items, _ := names.Items()
		for _, item := range items {
			name, _ := item.AsString()
			key = append(key, name)
		}
	}
	return NewMember(identity, key)
}

func fixtureAcquisition(record jsonx.Value) (Acquisition, error) {
	text, ok := editRecordText(record, "acquisition")
	if !ok {
		text = "conflict"
	}
	return ParseAcquisition(text)
}

func fixtureRequest(record jsonx.Value, format string) (DocumentRequest, error) {
	if format != "text" {
		return fixtureStructuredRequest(record, format)
	}
	selector, _ := record.Get("selector")
	boundaries, err := fixtureBoundaries(selector)
	if err != nil {
		return DocumentRequest{}, err
	}
	if action, _ := editRecordText(record, "action"); action == "retire" {
		claim, err := NewClaim(OwnershipSection, boundaries)
		if err != nil {
			return DocumentRequest{}, err
		}
		return NewRetireRequest(claim)
	}
	marker, ok := editRecordText(record, "marker")
	if !ok {
		marker = "fixture"
	}
	text, _ := editRecordText(record, "text")
	content, err := NewSectionContent(marker, text, &boundaries)
	if err != nil {
		return DocumentRequest{}, err
	}
	acquisition, err := fixtureAcquisition(record)
	if err != nil {
		return DocumentRequest{}, err
	}
	return NewSetRequest(content, acquisition)
}

func fixtureStructuredRequest(record jsonx.Value, format string) (DocumentRequest, error) {
	raw, _ := record.Get("selector")
	items, _ := raw.Items()
	parts := make([]SelectorPart, 0, len(items))
	for _, item := range items {
		if name, ok := item.AsString(); ok {
			key, err := NewKey(name)
			if err != nil {
				return DocumentRequest{}, err
			}
			parts = append(parts, key)
			continue
		}
		member, err := fixtureMember(item)
		if err != nil {
			return DocumentRequest{}, err
		}
		parts = append(parts, member)
	}
	selector, err := NewSelector(parts...)
	if err != nil {
		return DocumentRequest{}, err
	}
	if action, _ := editRecordText(record, "action"); action == "retire" {
		claim, err := NewClaim(OwnershipStructured, selector)
		if err != nil {
			return DocumentRequest{}, err
		}
		return NewRetireRequest(claim)
	}
	var value Value
	if semantic, ok := record.Get("semantic_value"); ok {
		value, err = valueFromRecord(semantic)
	} else {
		plain, _ := record.Get("value")
		value, err = valueFromJSON(plain)
	}
	if err != nil {
		return DocumentRequest{}, err
	}
	var placement *Placement
	if item, ok := record.Get("placement"); ok {
		text, _ := editRecordText(item, "position")
		position, err := ParsePosition(text)
		if err != nil {
			return DocumentRequest{}, err
		}
		var anchor *Member
		if raw, ok := item.Get("anchor"); ok {
			member, err := fixtureMember(raw)
			if err != nil {
				return DocumentRequest{}, err
			}
			anchor = &member
		}
		built, err := NewPlacement(position, anchor)
		if err != nil {
			return DocumentRequest{}, err
		}
		placement = &built
	}
	documentFormat, err := ParseDocumentFormat(format)
	if err != nil {
		return DocumentRequest{}, err
	}
	content, err := NewStructuredContent(documentFormat, selector, value, placement)
	if err != nil {
		return DocumentRequest{}, err
	}
	acquisition, err := fixtureAcquisition(record)
	if err != nil {
		return DocumentRequest{}, err
	}
	return NewSetRequest(content, acquisition)
}

func optionalFixtureText(record jsonx.Value, name string) ([]byte, bool) {
	text, ok := editRecordText(record, name)
	if !ok {
		return nil, false
	}
	return []byte(text), true
}

type takenBaseline struct {
	claim    Claim
	baseline *SelectionSnapshot
}

func TestEditFixturesMatchExpectedBytes(t *testing.T) {
	data, err := jsonx.Parse(fixture(t, "edits.json"))
	if err != nil {
		t.Fatal(err)
	}
	cases, _ := data.Get("cases")
	items, _ := cases.Items()
	ran := 0
	for _, item := range items {
		name, _ := editRecordText(item, "name")
		format, _ := editRecordText(item, "format")
		if !editFixtureFormats[format] {
			continue
		}
		ran++
		t.Run(name, func(t *testing.T) {
			runEditFixture(t, item, format)
		})
	}
	if ran == 0 {
		t.Fatal("edits.json has no runnable cases")
	}
	invalid, _ := data.Get("invalid")
	rejected, _ := invalid.Items()
	messages := map[string]string{
		"nested-custom-sections":   "nested or repeated section boundaries",
		"duplicate-json-key":       "duplicate JSON object key",
		"duplicate-jsonc-key":      "duplicate JSON object key",
		"duplicate-toml-key":       "Key \"x\" already exists. at line 2 col 0",
		"duplicate-array-identity": "duplicate or ambiguous array member identities",
		"overlapping-paths":        "document claims overlap",
	}
	for _, item := range rejected {
		name, _ := editRecordText(item, "name")
		format, _ := editRecordText(item, "format")
		if !editFixtureFormats[format] {
			continue
		}
		document, _ := editRecordText(item, "document")
		selectors, _ := item.Get("selectors")
		records, _ := selectors.Items()
		var requests []DocumentRequest
		for _, selector := range records {
			var request DocumentRequest
			if format == "text" {
				boundaries, err := fixtureBoundaries(selector)
				if err != nil {
					t.Fatalf("%s: %v", name, err)
				}
				request = mustSetRequest(t, mustSection(t, "fixture", "body", &boundaries), AcquireConflict)
			} else {
				record := jsonx.Object(
					jsonx.Member{Key: "action", Value: jsonx.String("set")},
					jsonx.Member{Key: "selector", Value: selector},
					jsonx.Member{Key: "value", Value: jsonx.Int(1)},
				)
				if request, err = fixtureStructuredRequest(record, format); err != nil {
					t.Fatalf("%s: %v", name, err)
				}
			}
			requests = append(requests, request)
		}
		_, err := EditDocument([]byte(document), true, requests, nil, DocumentProvenance{})
		want, known := messages[name]
		if !known {
			t.Fatalf("unexpected invalid case %s", name)
		}
		if got := errorText(err); got != want {
			t.Errorf("%s: error %q, want %q", name, got, want)
		}
	}
}

func runEditFixture(t *testing.T, item jsonx.Value, format string) {
	current, exists := optionalFixtureText(item, "before")
	var ownership []OwnedSelection
	var provenance DocumentProvenance
	var baselines []takenBaseline
	steps, _ := item.Get("steps")
	records, _ := steps.Items()
	for index, step := range records {
		observed, observedExists := current, exists
		if raw, ok := step.Get("observed"); ok {
			observed, observedExists = optionalFixtureText(step, "observed")
			if raw.IsNull() {
				observed, observedExists = nil, false
			}
		}
		requestRecords := []jsonx.Value{step}
		if raw, ok := step.Get("requests"); ok {
			requestRecords, _ = raw.Items()
		}
		requests := make([]DocumentRequest, 0, len(requestRecords))
		for _, record := range requestRecords {
			request, err := fixtureRequest(record, format)
			if err != nil {
				t.Fatalf("step %d: %v", index, err)
			}
			requests = append(requests, request)
		}
		result, err := EditDocument(observed, observedExists, requests, ownership, provenance)
		if err != nil {
			t.Fatalf("step %d: %v", index, err)
		}
		expected, expectedExists := optionalFixtureText(step, "expected")
		if result.AfterExists != expectedExists || !bytes.Equal(result.After, expected) {
			t.Fatalf("step %d: after = %q (exists %v), want %q (exists %v)", index, result.After, result.AfterExists, expected, expectedExists)
		}
		var wanted []EditStatus
		if raw, ok := step.Get("statuses"); ok {
			list, _ := raw.Items()
			for _, status := range list {
				text, _ := status.AsString()
				wanted = append(wanted, EditStatus(text))
			}
		} else {
			text, _ := editRecordText(step, "status")
			wanted = []EditStatus{EditStatus(text)}
		}
		if got := statuses(result); !slices.Equal(got, wanted) {
			t.Fatalf("step %d: statuses %v, want %v", index, got, wanted)
		}
		for _, owned := range result.Ownership {
			baseline, hasBaseline := owned.Baseline()
			if owned.Disposition() != DispositionTakenOver {
				if hasBaseline {
					t.Errorf("step %d: only takeover owns a baseline", index)
				}
				continue
			}
			known := slices.IndexFunc(baselines, func(item takenBaseline) bool { return item.claim.Equal(owned.Claim()) })
			if known < 0 {
				baselines = append(baselines, takenBaseline{claim: owned.Claim(), baseline: &baseline})
			} else if !baselines[known].baseline.equal(baseline) {
				t.Errorf("step %d: takeover baseline changed", index)
			}
		}
		if !result.Applicable() {
			if !ownershipEqual(result.Ownership, ownership) {
				t.Errorf("step %d: conflict changed ownership", index)
			}
			if !result.Provenance.equal(provenance) {
				t.Errorf("step %d: conflict changed provenance", index)
			}
		}
		current, exists = result.After, result.AfterExists
		ownership, provenance = result.Ownership, result.Provenance
	}
}

func TestSectionsPreserveForeignBytes(t *testing.T) {
	for _, newline := range newlines {
		for _, prefix := range []string{"", "foreign", "foreign\n", "foreign\n\n", "foreign\n\n\n"} {
			for _, body := range []string{"", "hello", "hello\n", "hello\n\n", " a \n b \r\n"} {
				raw := []byte("header" + newline + strings.ReplaceAll(prefix, "\n", newline))
				installed := setSection(t, raw, true, mustSection(t, "guide", body, nil), AcquireConflict)
				if !installed.Applicable() || !bytes.HasPrefix(installed.After, raw) {
					t.Errorf("prefix %q body %q: install changed foreign bytes: %q", prefix, body, installed.After)
				}
				expectAfter(t, "retire "+prefix+body, retireAll(t, installed, installed.After, true), raw)
				if installed.Ownership[0].Disposition() != DispositionCreated {
					t.Errorf("prefix %q body %q: disposition %s", prefix, body, installed.Ownership[0].Disposition())
				}
			}
		}
	}
}

func TestSectionRetirementPreservesAdjacentSectionsAndSuffix(t *testing.T) {
	for _, newline := range newlines {
		for _, retired := range [][]string{{"b"}, {"a", "b"}, {"b", "a"}} {
			prefix := []byte("Header" + newline)
			suffix := []byte("Üser suffix" + newline)
			installed := mustEdit(t, prefix, true, []DocumentRequest{
				mustSetRequest(t, mustSection(t, "a", "a", nil), AcquireConflict),
				mustSetRequest(t, mustSection(t, "b", "b", nil), AcquireConflict),
			}, nil, DocumentProvenance{})
			doubled := []byte("<!-- flyrail:a:end -->" + newline + newline)
			single := []byte("<!-- flyrail:a:end -->" + newline)
			observed := slices.Concat(bytes.ReplaceAll(installed.After, doubled, single), suffix)
			var requests []DocumentRequest
			for _, name := range retired {
				requests = append(requests, mustRetireRequest(t, sectionClaim(t, mustSection(t, name, name, nil))))
			}
			result := mustEdit(t, observed, true, requests, installed.Ownership, installed.Provenance)
			if !result.Applicable() || !slices.Equal(statuses(result), slices.Repeat([]EditStatus{EditRemoved}, len(retired))) {
				t.Fatalf("retired %v: statuses %v", retired, statuses(result))
			}
			if len(retired) == 2 {
				expectAfter(t, "both", result, slices.Concat(prefix, suffix))
				if len(result.Ownership) != 0 {
					t.Errorf("both retired but ownership remains: %d", len(result.Ownership))
				}
				continue
			}
			first := strings.ReplaceAll("<!-- flyrail:a:start -->\na\n<!-- flyrail:a:end -->\n", "\n", newline)
			expectAfter(t, "only b", result, slices.Concat(prefix, []byte(newline), []byte(first), suffix))
			again := updateSection(t, result, mustSection(t, "a", "a", nil), result.After, true)
			if again.Observations[0].Status != EditCurrent {
				t.Errorf("remaining section status %s", again.Observations[0].Status)
			}
			expectAfter(t, "remaining", again, result.After)
		}
	}
}

func TestSectionRetirementPreservesAnUnownedStandardNeighbor(t *testing.T) {
	for _, newline := range newlines {
		foreign := []byte(strings.ReplaceAll("<!-- flyrail:foreign:start -->\nUser text\n<!-- flyrail:foreign:end -->\n", "\n", newline))
		installed := setSection(t, foreign, true, mustSection(t, "owned", "generated", nil), AcquireConflict)
		doubled := []byte("<!-- flyrail:foreign:end -->" + newline + newline)
		single := []byte("<!-- flyrail:foreign:end -->" + newline)
		result := retireAll(t, installed, bytes.ReplaceAll(installed.After, doubled, single), true)
		if !result.Applicable() {
			t.Fatalf("newline %q: statuses %v", newline, statuses(result))
		}
		expectAfter(t, "neighbor", result, foreign)
	}
}

func TestSectionBOMTakeoverUpdateRenameAndRestore(t *testing.T) {
	for _, prefixNewline := range newlines {
		pair := mustBoundaries(t, "<!-- splashdown:start -->", "<!-- splashdown:end -->")
		prefix := []byte("\xef\xbb\xbfUser" + prefixNewline + prefixNewline)
		raw := slices.Concat(prefix, []byte("<!-- splashdown:start -->\r\n  old \r\n\r\n<!-- splashdown:end -->\r\nTAIL"))
		content := mustSection(t, "old-attribution", "new\n", pair)
		conflict := setSection(t, raw, true, content, AcquireConflict)
		expectAfter(t, "conflict", conflict, raw)
		if conflict.Observations[0].Status != EditConflict {
			t.Errorf("conflict status %s", conflict.Observations[0].Status)
		}
		first := setSection(t, raw, true, content, AcquireTakeover)
		expectAfter(t, "takeover", first, slices.Concat(prefix, []byte("<!-- splashdown:start -->\r\nnew\r\n<!-- splashdown:end -->\r\nTAIL")))
		renamed := mustSection(t, "renamed", "newer\n", pair)
		second := updateSection(t, first, renamed, first.After, true)
		firstBaseline, _ := first.Ownership[0].Baseline()
		secondBaseline, ok := second.Ownership[0].Baseline()
		if !ok || !secondBaseline.equal(firstBaseline) {
			t.Error("update replaced the takeover baseline")
		}
		if second.Observations[0].Status != EditUpdated {
			t.Errorf("update status %s", second.Observations[0].Status)
		}
		expectAfter(t, "restore", retireAll(t, second, second.After, true), raw)
		repeated := updateSection(t, second, renamed, second.After, true)
		if repeated.Observations[0].Status != EditCurrent {
			t.Errorf("repeat status %s", repeated.Observations[0].Status)
		}
	}
}

func TestModifiedSectionRetainsBaseline(t *testing.T) {
	original := []byte("Foreign\n<!-- flyrail:x:start -->\r\nold\r\n<!-- flyrail:x:end -->\r\n")
	content := mustSection(t, "x", "new\n", nil)
	first := setSection(t, original, true, content, AcquireTakeover)
	variants := map[string][]byte{
		"newline": bytes.ReplaceAll(first.After, []byte("\r\n"), []byte("\n")),
		"missing": []byte("foreign\n"),
		"body":    bytes.ReplaceAll(first.After, []byte("new"), []byte("edited")),
		"space":   bytes.ReplaceAll(first.After, []byte("new"), []byte("new ")),
	}
	for change, data := range variants {
		for label, result := range map[string]DocumentEdit{
			"update": updateSection(t, first, content, data, true),
			"remove": retireAll(t, first, data, true),
		} {
			expectAfter(t, label+" "+change, result, data)
			if !ownershipEqual(result.Ownership, first.Ownership) || !result.Provenance.equal(first.Provenance) {
				t.Errorf("%s %s: ownership or provenance changed", label, change)
			}
			if result.Observations[0].Status != EditModified {
				t.Errorf("%s %s: status %s", label, change, result.Observations[0].Status)
			}
		}
	}
}

func TestAdoptionIsExactAndRemovesWithoutABaseline(t *testing.T) {
	for _, prefixNewline := range newlines {
		for _, newline := range newlines {
			content := mustSection(t, "x", "generated\n", mustBoundaries(t, "START", "END"))
			prefix := []byte("before" + prefixNewline)
			suffix := []byte("after" + prefixNewline)
			raw := slices.Concat(prefix, []byte(strings.ReplaceAll("START\ngenerated\nEND\n", "\n", newline)), suffix)
			adopted := setSection(t, raw, true, content, AcquireAdopt)
			expectAfter(t, "adopt", adopted, raw)
			if _, hasBaseline := adopted.Ownership[0].Baseline(); adopted.Ownership[0].Disposition() != DispositionAdopted || hasBaseline {
				t.Errorf("adopted ownership %s (baseline %v)", adopted.Ownership[0].Disposition(), hasBaseline)
			}
			repeated := updateSection(t, adopted, content, adopted.After, true)
			if repeated.Observations[0].Status != EditCurrent || !ownershipEqual(repeated.Ownership, adopted.Ownership) {
				t.Errorf("repeat status %s", repeated.Observations[0].Status)
			}
			expectAfter(t, "repeat", repeated, raw)
			expectAfter(t, "remove", retireAll(t, repeated, repeated.After, true), slices.Concat(prefix, suffix))
			alternate := "\n"
			if newline == "\n" {
				alternate = "\r\n"
			}
			variants := [][]byte{
				bytes.ReplaceAll(raw, []byte("generated"), []byte("generated ")),
				bytes.ReplaceAll(raw, []byte("generated"), []byte("generated"+newline)),
				bytes.ReplaceAll(raw, []byte("generated"), []byte("GENERATED")),
				bytes.ReplaceAll(raw, []byte("generated"+newline), []byte("generated"+alternate)),
				bytes.ReplaceAll(raw, []byte("END"+newline), []byte("END"+alternate)),
			}
			for _, variant := range variants {
				result := setSection(t, variant, true, content, AcquireAdopt)
				if result.Observations[0].Status != EditConflict {
					t.Errorf("variant %q: status %s", variant, result.Observations[0].Status)
				}
				expectAfter(t, "variant", result, variant)
			}
		}
	}
}

func TestSectionsUseTheirOwnNewlinesAndNewSectionsUseTheDocument(t *testing.T) {
	raw := []byte("Header\nA\r\nold\r\nEND A\r\nB\rkeep\rEND B\rFooter\n")
	result := mustEdit(t, raw, true, []DocumentRequest{
		mustSetRequest(t, mustSection(t, "a", "new", mustBoundaries(t, "A", "END A")), AcquireTakeover),
		mustSetRequest(t, mustSection(t, "b", "keep", mustBoundaries(t, "B", "END B")), AcquireAdopt),
		mustSetRequest(t, mustSection(t, "c", "created", nil), AcquireConflict),
	}, nil, DocumentProvenance{})
	if got := statuses(result); !slices.Equal(got, []EditStatus{EditTakenOver, EditAdopted, EditCreated}) {
		t.Errorf("statuses %v", got)
	}
	expectAfter(t, "edit", result, []byte("Header\nA\r\nnew\r\nEND A\r\nB\rkeep\rEND B\rFooter\n\n<!-- flyrail:c:start -->\ncreated\n<!-- flyrail:c:end -->\n"))
	expectAfter(t, "remove", retireAll(t, result, result.After, true), []byte("Header\nA\r\nold\r\nEND A\r\nFooter\n"))
}

func TestMalformedOrIntersectingSectionsAreRejected(t *testing.T) {
	requests := []DocumentRequest{
		mustSetRequest(t, mustSection(t, "x", "new", mustBoundaries(t, "START", "END")), AcquireConflict),
		mustSetRequest(t, mustSection(t, "y", "other", mustBoundaries(t, "OTHER", "STOP")), AcquireConflict),
	}
	cases := map[string]string{
		"START\n":                             "section has no closing boundary",
		"END\n":                               "malformed or intersecting section boundaries",
		"END\nSTART\n":                        "malformed or intersecting section boundaries",
		"START\nx\nSTART\nEND\n":              "nested or repeated section boundaries",
		"START\nEND\nSTART\nEND\n":            "nested or repeated section boundaries",
		"prefix START\nEND\n":                 "section boundaries must occupy complete lines",
		"START\nEND suffix\n":                 "section boundaries must occupy complete lines",
		"START\nOTHER\nEND\nSTOP\n":           "nested or repeated section boundaries",
		"START\nOTHER\nSTOP\nEND\n":           "nested or repeated section boundaries",
		"<!-- flyrail:bad marker:start -->\n": "section marker must be 1-64 lowercase ASCII letters/digits with internal hyphens",
	}
	for raw, want := range cases {
		_, err := EditDocument([]byte(raw), true, requests, nil, DocumentProvenance{})
		var invalid *ValueError
		if !errors.As(err, &invalid) {
			t.Errorf("raw %q: error %v, want a ValueError", raw, err)
			continue
		}
		if invalid.Message != want {
			t.Errorf("raw %q: error %q, want %q", raw, invalid.Message, want)
		}
	}
}

func TestUnrequestedNativeSectionsAreAlsoCheckedForIntersection(t *testing.T) {
	raw := []byte("<!-- flyrail:foreign:start -->\nSTART\n<!-- flyrail:foreign:end -->\nEND\n")
	request := mustSetRequest(t, mustSection(t, "x", "new", mustBoundaries(t, "START", "END")), AcquireTakeover)
	_, err := EditDocument(raw, true, []DocumentRequest{request}, nil, DocumentProvenance{})
	if got := errorText(err); got != "nested or repeated section boundaries" {
		t.Errorf("error %q", got)
	}
}

func TestSectionsMultiownerCreationAndSeparators(t *testing.T) {
	a := setSection(t, nil, false, mustSection(t, "a", "A", nil), AcquireConflict)
	b := updateSection(t, a, mustSection(t, "b", "B", nil), a.After, a.AfterExists)
	c := mustEdit(t, b.After, true, []DocumentRequest{mustRetireRequest(t, a.Ownership[0].Claim())}, b.Ownership, b.Provenance)
	expectAfter(t, "retire a", c, []byte("\n<!-- flyrail:b:start -->\nB\n<!-- flyrail:b:end -->\n"))
	if empty, ok := c.Provenance.EmptyDocument(); !ok || len(empty) != 0 {
		t.Errorf("empty document provenance = %q, %v", empty, ok)
	}
	if final := retireAll(t, c, c.After, true); final.AfterExists {
		t.Errorf("retiring every section kept %q", final.After)
	}
	foreign := updateSection(t, a, mustSection(t, "b", "B", nil), slices.Concat([]byte("User\n"), a.After), true)
	final := retireAll(t, foreign, foreign.After, true)
	expectAfter(t, "foreign", final, []byte("User\n"))
	if !final.Provenance.equal(DocumentProvenance{}) {
		t.Error("final provenance is not empty")
	}
}

func TestSeparatorIsRetainedWhenNoLongerAdjacent(t *testing.T) {
	first := setSection(t, []byte("foreign"), true, mustSection(t, "x", "body", nil), AcquireConflict)
	data := bytes.ReplaceAll(first.After, []byte("\n\n<!--"), []byte("\n\nUSER\n<!--"))
	expectAfter(t, "retire", retireAll(t, first, data, true), []byte("foreign\n\nUSER\n"))
}

func TestSectionResourceConflictsAreAtomic(t *testing.T) {
	a := setSection(t, nil, false, mustSection(t, "a", "A", nil), AcquireConflict)
	data := bytes.ReplaceAll(a.After, []byte("\nA\n"), []byte("\nUSER\n"))
	result := mustEdit(t, data, true, []DocumentRequest{
		mustRetireRequest(t, a.Ownership[0].Claim()),
		mustSetRequest(t, mustSection(t, "b", "B", nil), AcquireConflict),
	}, a.Ownership, a.Provenance)
	expectAfter(t, "conflict", result, data)
	if result.Applicable() || !ownershipEqual(result.Ownership, a.Ownership) {
		t.Errorf("statuses %v changed ownership", statuses(result))
	}
}

func structuredKeys(t *testing.T, format DocumentFormat, value Value, names ...string) StructuredContent {
	t.Helper()
	parts := make([]SelectorPart, 0, len(names))
	for _, name := range names {
		key, err := NewKey(name)
		if err != nil {
			t.Fatal(err)
		}
		parts = append(parts, key)
	}
	selector, err := NewSelector(parts...)
	if err != nil {
		t.Fatal(err)
	}
	content, err := NewStructuredContent(format, selector, value, nil)
	if err != nil {
		t.Fatal(err)
	}
	return content
}

func TestConflictingClaimsAreRejectedBeforeChanges(t *testing.T) {
	empty, err := NewObject()
	if err != nil {
		t.Fatal(err)
	}
	parent := structuredKeys(t, FormatJSON, empty, "mcp")
	child := structuredKeys(t, FormatJSON, Int(1), "mcp", "x")
	other := structuredKeys(t, FormatJSONC, Int(2), "y")
	section := mustSection(t, "x", "body", nil)
	rows := []struct {
		requests []DocumentRequest
		message  string
	}{
		{[]DocumentRequest{mustSetRequest(t, parent, AcquireConflict), mustSetRequest(t, child, AcquireConflict)}, "document claims overlap"},
		{[]DocumentRequest{mustSetRequest(t, child, AcquireConflict), mustSetRequest(t, child, AcquireConflict)}, "duplicate document claim"},
		{[]DocumentRequest{mustSetRequest(t, child, AcquireConflict), mustSetRequest(t, section, AcquireConflict)}, "document claims overlap"},
		{[]DocumentRequest{mustSetRequest(t, child, AcquireConflict), mustSetRequest(t, other, AcquireConflict)}, "one document must use one editor format"},
		{[]DocumentRequest{
			mustSetRequest(t, mustSection(t, "a", "A", mustBoundaries(t, "START", "END")), AcquireConflict),
			mustSetRequest(t, mustSection(t, "b", "B", mustBoundaries(t, "START", "STOP")), AcquireConflict),
		}, "document claims overlap"},
	}
	for _, row := range rows {
		_, err := EditDocument(nil, false, row.requests, nil, DocumentProvenance{})
		if got := errorText(err); got != row.message {
			t.Errorf("error %q, want %q", got, row.message)
		}
	}
}

func markerClaim(t *testing.T, marker string) Claim {
	t.Helper()
	claim, err := NewClaim(OwnershipSection, SubtreeSelector(marker))
	if err != nil {
		t.Fatal(err)
	}
	return claim
}

func TestDocumentRequestsValidateLikePython(t *testing.T) {
	fileClaim, err := NewClaim(OwnershipFile, WholeSelector{})
	if err != nil {
		t.Fatal(err)
	}
	file, err := NewFileContent([]byte("x"), ModeReadable)
	if err != nil {
		t.Fatal(err)
	}
	_, nonDocument := NewDocumentRequest(fileClaim, nil, AcquireConflict)
	_, wrongContent := NewSetRequest(file, AcquireConflict)
	_, mismatched := NewDocumentRequest(markerClaim(t, "a"), mustSection(t, "b", "text", nil), AcquireConflict)
	_, acquiring := NewDocumentRequest(markerClaim(t, "a"), nil, AcquireAdopt)
	_, zeroClaim := NewRetireRequest(Claim{})
	_, badAcquisition := NewSetRequest(mustSection(t, "a", "text", nil), Acquisition("grab"))
	cases := []struct {
		err  error
		want string
	}{
		{nonDocument, "document requests require section or structured claims"},
		{wrongContent, "document content must be a section or structured selection"},
		{mismatched, "requested content must address its physical claim"},
		{acquiring, "retirement does not acquire ownership"},
		{zeroClaim, "document requests require section or structured claims"},
		{badAcquisition, "'grab' is not a valid Acquisition"},
	}
	for _, c := range cases {
		if got := errorText(c.err); got != c.want {
			t.Errorf("error %q, want %q", got, c.want)
		}
	}
	request, err := NewDocumentRequest(markerClaim(t, "a"), mustSection(t, "a", "text", nil), "")
	if err != nil {
		t.Fatal(err)
	}
	if request.Acquisition() != AcquireConflict || !request.Claim().Equal(markerClaim(t, "a")) {
		t.Errorf("request = %s %v", request.Acquisition(), request.Claim())
	}
	if content, ok := request.Content().(SectionContent); !ok || content.Text() != "text" {
		t.Errorf("content = %#v", request.Content())
	}
}

func TestAcquisitionParsing(t *testing.T) {
	for _, want := range []Acquisition{AcquireConflict, AcquireTakeover, AcquireAdopt} {
		if got, err := ParseAcquisition(string(want)); err != nil || got != want {
			t.Errorf("ParseAcquisition(%q) = %q, %v", want, got, err)
		}
	}
	if _, err := ParseAcquisition("Adopt"); errorText(err) != "'Adopt' is not a valid Acquisition" {
		t.Errorf("error %v", err)
	}
}

func TestNoRequestsPreserveDocumentsAndRejectBadBytes(t *testing.T) {
	expectAfter(t, "unchanged", mustEdit(t, []byte("unchanged"), true, nil, nil, DocumentProvenance{}), []byte("unchanged"))
	if absent := mustEdit(t, []byte("ignored"), false, nil, nil, DocumentProvenance{}); absent.AfterExists || absent.BeforeExists {
		t.Errorf("absent document = %+v", absent)
	}
	cases := []struct{ data, want string }{
		{"\xff", "'utf-8' codec can't decode byte 0xff in position 0: invalid start byte"},
		{"\xe0\xbf\x00", "'utf-8' codec can't decode bytes in position 0-1: invalid continuation byte"},
		{"ok\xe0\xbf", "'utf-8' codec can't decode bytes in position 2-3: unexpected end of data"},
		{"\xf0", "'utf-8' codec can't decode byte 0xf0 in position 0: unexpected end of data"},
		{"a\xed\xa0\x80", "'utf-8' codec can't decode byte 0xed in position 1: invalid continuation byte"},
		{"\xc0\x80", "'utf-8' codec can't decode byte 0xc0 in position 0: invalid start byte"},
		{"\xf4\x90\x80\x80", "'utf-8' codec can't decode byte 0xf4 in position 0: invalid continuation byte"},
		{"\xef\xbb\xbf\xf0\x9f\x98", "'utf-8' codec can't decode bytes in position 0-2: unexpected end of data"},
		{"bad\x00", "documents cannot contain NUL"},
	}
	for _, c := range cases {
		_, err := EditDocument([]byte(c.data), true, nil, nil, DocumentProvenance{})
		if got := errorText(err); got != c.want {
			t.Errorf("data %q: error %q, want %q", c.data, got, c.want)
		}
	}
	section := setSection(t, nil, false, mustSection(t, "x", "body", nil), AcquireConflict)
	observed := mustEdit(t, nil, false, nil, section.Ownership, section.Provenance)
	if observed.AfterExists || !ownershipEqual(observed.Ownership, section.Ownership) || !observed.Provenance.equal(section.Provenance) {
		t.Errorf("observation without requests changed state: %+v", observed)
	}
}

func TestDuplicateClaimsAndRetirementEdgesMatchPython(t *testing.T) {
	section := setSection(t, nil, false, mustSection(t, "x", "body", nil), AcquireConflict)
	owned := section.Ownership[0]
	_, err := EditDocument(section.After, true, nil, []OwnedSelection{owned, owned}, DocumentProvenance{})
	if got := errorText(err); got != "duplicate document claim" {
		t.Errorf("duplicate ownership error %q", got)
	}
	key, err := NewKey("x")
	if err != nil {
		t.Fatal(err)
	}
	selector, err := NewSelector(key)
	if err != nil {
		t.Fatal(err)
	}
	structured, err := NewClaim(OwnershipStructured, selector)
	if err != nil {
		t.Fatal(err)
	}
	_, err = EditDocument(nil, false, []DocumentRequest{mustRetireRequest(t, structured)}, nil, DocumentProvenance{})
	if got := errorText(err); got != "retirement requires existing ownership and its document format" {
		t.Errorf("structured retirement error %q", got)
	}
	unowned := mustEdit(t, []byte("foreign"), true, []DocumentRequest{mustRetireRequest(t, markerClaim(t, "x"))}, nil, DocumentProvenance{})
	if unowned.Observations[0].Status != EditConflict {
		t.Errorf("unowned retirement status %s", unowned.Observations[0].Status)
	}
	expectAfter(t, "unowned", unowned, []byte("foreign"))
}

func TestZeroValueEditorInputsAreRejected(t *testing.T) {
	section := setSection(t, nil, false, mustSection(t, "x", "body", nil), AcquireConflict)
	_, err := EditDocument(nil, false, []DocumentRequest{{}}, nil, DocumentProvenance{})
	if got := errorText(err); got != "document requests require section or structured claims" {
		t.Errorf("zero request error %q", got)
	}
	_, err = EditDocument(section.After, true, nil, []OwnedSelection{{}}, DocumentProvenance{})
	if got := errorText(err); got != "ownership requires a Claim" {
		t.Errorf("zero ownership error %q", got)
	}
}

func TestEditResultsDoNotAliasInputs(t *testing.T) {
	source := []byte("Header\n")
	first := setSection(t, source, true, mustSection(t, "x", "body", nil), AcquireConflict)
	source[0] = 'X'
	if !bytes.Equal(first.Before, []byte("Header\n")) || !bytes.HasPrefix(first.After, []byte("Header\n")) {
		t.Errorf("result aliases the caller's source: before %q after %q", first.Before, first.After)
	}
	unchanged := mustEdit(t, []byte("same"), true, nil, nil, DocumentProvenance{})
	unchanged.After[0] = 'X'
	if !bytes.Equal(unchanged.Before, []byte("same")) {
		t.Errorf("before aliases after: %q", unchanged.Before)
	}
	snapshot := slices.Clone(first.Ownership)
	second := updateSection(t, first, mustSection(t, "x", "changed", nil), first.After, true)
	if second.Observations[0].Status != EditUpdated || !ownershipEqual(first.Ownership, snapshot) {
		t.Errorf("update changed the previous result: %v", statuses(second))
	}
}

func TestObserveSelectionsReportsSectionState(t *testing.T) {
	installed := mustEdit(t, []byte("Header\n"), true, []DocumentRequest{
		mustSetRequest(t, mustSection(t, "a", "A", nil), AcquireConflict),
		mustSetRequest(t, mustSection(t, "b", "B", nil), AcquireConflict),
	}, nil, DocumentProvenance{})
	edited := bytes.ReplaceAll(installed.After, []byte("\nB\n"), []byte("\nUSER\n"))
	observations, err := observeSelections(document{data: edited, exists: true}, installed.Ownership)
	if err != nil {
		t.Fatal(err)
	}
	got := make([]EditStatus, 0, len(observations))
	for _, item := range observations {
		got = append(got, item.Status)
	}
	if !slices.Equal(got, []EditStatus{EditCurrent, EditModified}) {
		t.Errorf("statuses %v", got)
	}
	current, ok := observations[0].Current.Value().(SnapshotBytes)
	if !ok || string(current.Bytes()) != "<!-- flyrail:a:start -->\nA\n<!-- flyrail:a:end -->\n" {
		t.Errorf("current snapshot %#v", observations[0].Current)
	}
	missing, err := observeSelections(document{}, installed.Ownership)
	if err != nil {
		t.Fatal(err)
	}
	if missing[0].Status != EditModified || missing[0].Current != nil {
		t.Errorf("absent document observation %+v", missing[0])
	}
}
