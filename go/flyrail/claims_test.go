package flyrail

import (
	"errors"
	"testing"

	"github.com/nielsmadan/flyrail/go/internal/jsonx"
	"github.com/nielsmadan/flyrail/go/internal/pathx"
)

type configurationFixture struct {
	raw jsonx.Value
}

func configurations(t *testing.T) configurationFixture {
	t.Helper()
	raw, err := jsonx.Parse(fixture(t, "configurations.json"))
	if err != nil {
		t.Fatal(err)
	}
	return configurationFixture{raw: raw}
}

func (f configurationFixture) cases(t *testing.T, name string) []jsonx.Value {
	t.Helper()
	section, _ := f.raw.Get(name)
	items, ok := section.Items()
	if !ok {
		t.Fatalf("configurations.json is missing %s", name)
	}
	return items
}

func text(t *testing.T, value jsonx.Value, key string) string {
	t.Helper()
	item, _ := value.Get(key)
	result, ok := item.AsString()
	if !ok {
		t.Fatalf("fixture field %s is not a string", key)
	}
	return result
}

func claimVector(t *testing.T, kind string, selector jsonx.Value) Claim {
	t.Helper()
	ownership := must(ParseOwnership(kind))
	if selector.IsNull() {
		return must(NewClaim(ownership, WholeSelector{}))
	}
	if path, ok := selector.AsString(); ok {
		return must(NewClaim(ownership, SubtreeSelector(path)))
	}
	items, _ := selector.Items()
	if len(items) == 2 {
		start, startText := items[0].AsString()
		end, endText := items[1].AsString()
		if startText && endText {
			return must(NewClaim(ownership, must(NewSectionBoundaries(start, end))))
		}
	}
	parts := make([]SelectorPart, 0, len(items))
	for _, item := range items {
		part, _ := item.Items()
		switch tag, _ := part[0].AsString(); tag {
		case "key":
			name, _ := part[1].AsString()
			parts = append(parts, must(NewKey(name)))
		case "member":
			rawKeys, _ := part[1].Items()
			keys := make([]string, 0, len(rawKeys))
			for _, key := range rawKeys {
				name, _ := key.AsString()
				keys = append(keys, name)
			}
			parts = append(parts, must(NewMember(must(valueFromRecord(part[2])), keys)))
		default:
			t.Fatalf("invalid claim vector part %s", tag)
		}
	}
	return must(NewClaim(ownership, must(NewSelector(parts...))))
}

func fixtureClaims(t *testing.T, f configurationFixture) map[string]Claim {
	t.Helper()
	claims := map[string]Claim{}
	for _, item := range f.cases(t, "claims") {
		selector, _ := item.Get("selector")
		claims[text(t, item, "name")] = claimVector(t, text(t, item, "kind"), selector)
	}
	return claims
}

func TestClaimIdentityFixtureVectors(t *testing.T) {
	f := configurations(t)
	claims := fixtureClaims(t, f)
	for _, item := range f.cases(t, "claims") {
		name := text(t, item, "name")
		identity, err := claims[name].identity(text(t, item, "destination"))
		if err != nil {
			t.Fatalf("%s: %v", name, err)
		}
		if expected := text(t, item, "identity"); identity != expected {
			t.Errorf("%s: identity = %s, want %s", name, identity, expected)
		}
	}
}

func TestClaimOverlapFixtureVectors(t *testing.T) {
	f := configurations(t)
	claims := fixtureClaims(t, f)
	for _, item := range f.cases(t, "claim_overlaps") {
		first, second := text(t, item, "first"), text(t, item, "second")
		raw, _ := item.Get("overlaps")
		expected, _ := raw.AsBool()
		if got := claims[first].Overlaps(claims[second]); got != expected {
			t.Errorf("%s overlaps %s = %v, want %v", first, second, got, expected)
		}
		if got := claims[second].Overlaps(claims[first]); got != expected {
			t.Errorf("%s overlaps %s = %v, want %v", second, first, got, expected)
		}
	}
}

func TestAuthorityFixtureVectors(t *testing.T) {
	for _, item := range configurations(t).cases(t, "authorities") {
		name, destination := text(t, item, "name"), text(t, item, "destination")
		if got, want := destinationKey(destination), text(t, item, "destination_key"); got != want {
			t.Errorf("%s: destination key = %s, want %s", name, got, want)
		}
		path, err := TransactionPath(destination, text(t, item, "transaction_id"))
		if err != nil {
			t.Fatalf("%s: %v", name, err)
		}
		if want := text(t, item, "transaction_path"); path != want {
			t.Errorf("%s: transaction path = %s, want %s", name, path, want)
		}
	}
}

func TestStateKeysFollowTheSharedScheme(t *testing.T) {
	if got := indexKey("team", "abc"); got != "index/team/abc" {
		t.Errorf("indexKey = %s", got)
	}
	if got, want := destinationKey("/tmp/AGENTS.md"), sha256Hex([]byte("/tmp/agents.md")); got != want {
		t.Errorf("destinationKey = %s, want %s", got, want)
	}
	if got, want := resourceKey("/tmp/AGENTS.md", "receipt"), "resource/"+destinationKey("/tmp/agents.md")+"/receipt"; got != want {
		t.Errorf("resourceKey = %s, want %s", got, want)
	}
}

func TestTransactionPathPlacesTheDirectoryBesideTheDestination(t *testing.T) {
	cases := []struct {
		destination string
		parent      string
	}{
		{"/a/b/AGENTS.md", "/a/b/"},
		{"/AGENTS.md", "/"},
		{"/a/b/", "/a/"},
		{"AGENTS.md", ""},
		{"x/AGENTS.md", "x/"},
	}
	id := "0123456789abcdef0123456789abcdef"
	for _, c := range cases {
		path, err := TransactionPath(c.destination, id)
		if err != nil {
			t.Fatal(err)
		}
		name, _ := pathx.FileName(c.destination)
		want := c.parent + ".flyrail-" + sha256Hex([]byte(portablePathKey(name))) + "-" + id + ".txn"
		if path != want {
			t.Errorf("TransactionPath(%q) = %q, want %q", c.destination, path, want)
		}
	}
	for _, bad := range []string{"", "0123456789ABCDEF0123456789ABCDEF", "0123456789abcdef0123456789abcde"} {
		_, err := TransactionPath("/a/b", bad)
		if err == nil || err.Error() != "transaction id must be 32 lowercase hexadecimal characters" {
			t.Errorf("TransactionPath(%q) error = %v", bad, err)
		}
	}
}

func TestNewClaimValidatesSelectorsAsRust(t *testing.T) {
	selector := must(NewSelector(must(NewKey("a"))))
	cases := []struct {
		kind      Ownership
		selector  ClaimSelector
		message   string
		typeError bool
	}{
		{OwnershipFile, SubtreeSelector("x"), "whole-resource claims cannot have selectors", false},
		{OwnershipTree, selector, "whole-resource claims cannot have selectors", false},
		{OwnershipStructured, WholeSelector{}, "structured claims require a Selector", true},
		{OwnershipStructured, Selector{}, "a selector must select a key or member", false},
		{OwnershipSection, WholeSelector{}, "section claims require SectionBoundaries or a marker name", true},
		{OwnershipSection, SectionBoundaries{}, "section boundaries must be nonblank single lines without NUL", false},
		{OwnershipSection, SubtreeSelector("Bad"), "section marker must be 1-64 lowercase ASCII letters/digits with internal hyphens", false},
		{OwnershipSubtree, WholeSelector{}, "subtree claims require a string selector", true},
		{OwnershipSubtree, SubtreeSelector("../x"), "subtree selector must be an unambiguous relative path", false},
		{OwnershipSubtree, nil, "subtree claims require a string selector", true},
		{"bogus", WholeSelector{}, "'bogus' is not a valid Ownership", false},
	}
	for _, c := range cases {
		_, err := NewClaim(c.kind, c.selector)
		failure, ok := errors.AsType[*ValueError](err)
		if !ok || failure.Message != c.message || failure.pythonTypeError != c.typeError {
			t.Errorf("NewClaim(%q, %#v) = %v, want %q (type error %v)", c.kind, c.selector, err, c.message, c.typeError)
		}
	}
}

func TestSectionClaimConvertsAMarkerToBoundaries(t *testing.T) {
	claim := must(NewClaim(OwnershipSection, SubtreeSelector("guide")))
	want := must(SectionBoundariesFromMarker("guide"))
	if got, ok := claim.Selector().(SectionBoundaries); !ok || got != want {
		t.Errorf("selector = %#v, want %#v", claim.Selector(), want)
	}
	if !claim.Equal(must(NewClaim(OwnershipSection, want))) {
		t.Error("marker and explicit boundary claims differ")
	}
}

func TestClaimEqualComparesKindAndSelector(t *testing.T) {
	member := must(NewMember(must(NewString("x")), []string{"name"}))
	structured := must(NewClaim(OwnershipStructured, must(NewSelector(must(NewKey("a")), member))))
	same := must(NewClaim(OwnershipStructured, must(NewSelector(must(NewKey("a")), member))))
	other := must(NewClaim(OwnershipStructured, must(NewSelector(must(NewKey("b")), member))))
	file := must(NewClaim(OwnershipFile, WholeSelector{}))
	tree := must(NewClaim(OwnershipTree, WholeSelector{}))
	cases := []struct {
		left, right Claim
		equal       bool
	}{
		{structured, same, true},
		{structured, other, false},
		{file, must(NewClaim(OwnershipFile, WholeSelector{})), true},
		{file, tree, false},
		{must(NewClaim(OwnershipSubtree, SubtreeSelector("a"))), must(NewClaim(OwnershipSubtree, SubtreeSelector("a"))), true},
		{must(NewClaim(OwnershipSubtree, SubtreeSelector("a"))), must(NewClaim(OwnershipSubtree, SubtreeSelector("A"))), false},
	}
	for _, c := range cases {
		if got := c.left.Equal(c.right); got != c.equal {
			t.Errorf("%#v.Equal(%#v) = %v, want %v", c.left, c.right, got, c.equal)
		}
	}
}

func TestSubtreeClaimsOverlapByPortableKey(t *testing.T) {
	subtree := func(path string) Claim { return must(NewClaim(OwnershipSubtree, SubtreeSelector(path))) }
	cases := []struct {
		left, right string
		overlaps    bool
	}{
		{"Review", "review/scripts", true},
		{"review", "reviewer", false},
		{"Straße", "STRASSE", true},
	}
	for _, c := range cases {
		if got := subtree(c.left).Overlaps(subtree(c.right)); got != c.overlaps {
			t.Errorf("%s overlaps %s = %v, want %v", c.left, c.right, got, c.overlaps)
		}
	}
}

func TestContentClaimAddressesSectionsAndStructuredContent(t *testing.T) {
	section := must(NewSectionContent("guide", "text", nil))
	claim := must(ContentClaim(section))
	if claim.Kind() != OwnershipSection || !claim.Equal(must(NewClaim(OwnershipSection, SubtreeSelector("guide")))) {
		t.Errorf("section content claim = %#v", claim)
	}
	selector := must(NewSelector(must(NewKey("a"))))
	structured := must(NewStructuredContent(FormatJSON, selector, must(NewString("v")), nil))
	if claim := must(ContentClaim(structured)); !claim.Equal(must(NewClaim(OwnershipStructured, selector))) {
		t.Errorf("structured content claim = %#v", claim)
	}
	_, err := ContentClaim(must(NewFileContent([]byte("x"), ModeReadable)))
	if failure, ok := errors.AsType[*ValueError](err); !ok || failure.Message != "document content must be a section or structured selection" {
		t.Errorf("file content claim error = %v", err)
	}
}
