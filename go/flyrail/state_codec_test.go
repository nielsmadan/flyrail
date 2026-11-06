package flyrail

import (
	"bytes"
	"encoding/hex"
	"errors"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"

	"github.com/nielsmadan/flyrail/go/internal/jsonx"
)

type stateVector struct {
	name     string
	typeName string
	data     []byte
	digest   string
}

func stateVectors(t testing.TB) []stateVector {
	t.Helper()
	data, err := os.ReadFile(filepath.Join("testdata", "state_vectors.tsv"))
	if err != nil {
		t.Fatal(err)
	}
	var vectors []stateVector
	for line := range strings.Lines(string(data)) {
		line = strings.TrimSuffix(line, "\n")
		if strings.HasPrefix(line, "#") {
			continue
		}
		fields := strings.Split(line, "\t")
		if len(fields) != 4 {
			t.Fatalf("malformed state vector line %q", line)
		}
		record, err := hex.DecodeString(fields[2])
		if err != nil {
			t.Fatal(err)
		}
		vectors = append(vectors, stateVector{name: fields[0], typeName: fields[1], data: record, digest: fields[3]})
	}
	return vectors
}

func roundTrip[T stateRecord](data []byte, from func(jsonx.Value) (T, error)) ([]byte, error) {
	value, err := decodeState(data, from)
	if err != nil {
		return nil, err
	}
	return encodeRecord(value), nil
}

var stateTypeNames = []string{
	"Claim", "DocumentSchema", "Key", "Member", "SectionBoundaries", "Selector", "ArrayPosition",
	"CreatedContainer", "DocumentProvenance", "OwnedSelection", "SelectionSnapshot", "Ancestor", "Node",
	"Revision", "ResourceRef", "OwnedClaim", "Receipt", "Generation", "Index", "Journal", "Preparation",
}

func decodeAs(typeName string, data []byte) ([]byte, error) {
	switch typeName {
	case "Claim":
		return roundTrip(data, claimFromState)
	case "DocumentSchema":
		return roundTrip(data, documentSchemaFromState)
	case "Key":
		return roundTrip(data, keyFromState)
	case "Member":
		return roundTrip(data, memberFromState)
	case "SectionBoundaries":
		return roundTrip(data, sectionBoundariesFromState)
	case "Selector":
		return roundTrip(data, selectorFromState)
	case "ArrayPosition":
		return roundTrip(data, arrayPositionFromState)
	case "CreatedContainer":
		return roundTrip(data, createdContainerFromState)
	case "DocumentProvenance":
		return roundTrip(data, documentProvenanceFromState)
	case "OwnedSelection":
		return roundTrip(data, ownedSelectionFromState)
	case "SelectionSnapshot":
		return roundTrip(data, selectionSnapshotFromState)
	case "Ancestor":
		return roundTrip(data, ancestorFromState)
	case "Node":
		return roundTrip(data, nodeFromState)
	case "Revision":
		return roundTrip(data, revisionFromState)
	case "ResourceRef":
		return roundTrip(data, resourceRefFromState)
	case "OwnedClaim":
		return roundTrip(data, ownedClaimFromState)
	case "Receipt":
		return roundTrip(data, receiptFromState)
	case "Generation":
		return roundTrip(data, generationFromState)
	case "Index":
		return roundTrip(data, indexFromState)
	case "Journal":
		return roundTrip(data, journalFromState)
	case "Preparation":
		return roundTrip(data, preparationFromState)
	}
	return nil, errors.New("unhandled state type " + typeName)
}

func decodeOutcome(typeName string, data []byte) string {
	_, err := decodeAs(typeName, data)
	if err != nil {
		return err.Error()
	}
	return ""
}

func TestStateVectorsRoundTripByteIdentically(t *testing.T) {
	vectors := stateVectors(t)
	if len(vectors) != 31 {
		t.Fatalf("found %d state vectors, want 31", len(vectors))
	}
	for _, vector := range vectors {
		encoded, err := decodeAs(vector.typeName, vector.data)
		if err != nil {
			t.Errorf("%s: %v", vector.name, err)
			continue
		}
		if !bytes.Equal(encoded, vector.data) {
			t.Errorf("%s: re-encoded\n%s\nwant\n%s", vector.name, encoded, vector.data)
		}
		if got := sha256Hex(encoded); got != vector.digest {
			t.Errorf("%s: digest %s, want %s", vector.name, got, vector.digest)
		}
	}
}

func TestStateVectorsMatchTheRustDigests(t *testing.T) {
	rust := map[string]string{
		"fixture-receipt-created-section":         "1707a7b5e01b8cbdfb38a4c1672d3d194d7d71f6f5f0de177f587c99a24a6173",
		"fixture-receipt-taken-over-section":      "bdb08a71aaafc8659817ee1454b32de3f13646abf06a6e44739097ea7a71ef81",
		"fixture-receipt-adopted-section":         "13a94f0567e83a75ca6ab68f5d536e8d1afc0f1db5f056064829260e5117e722",
		"fixture-receipt-taken-over-member":       "70b2a778afb80feb8d6ef7e83e0a7d5fedc57a91c8a8085d01e121c6e7b23121",
		"fixture-receipt-created-document-schema": "7132ca049258fa097bb8c0f980500f23c443e262e9631714f94c20c890103b75",
		"fixture-receipt-empty-document-schema":   "428ec3b7a5c5f401c3b5a776fadedeba98bc7134d46b17aa8ac30d67e0de4204",
		"fixture-journal-create-section":          "1047c6c1919a5f496fd893445e98353cde1cae99c24dce84bb5a4f40aa8119b9",
		"fixture-journal-adopt-section":           "9b7ea27d2cfcbacbb89d2082dd28e72c7b3a8e00e978407abf2e3af9bcc7116b",
		"fixture-preparation-create-section":      "c392d66b505eafb807a5cb156e64667127aefcc9d5a59d8588740c57133c003b",
	}
	found := 0
	for _, vector := range stateVectors(t) {
		if want, ok := rust[vector.name]; ok {
			found++
			if vector.digest != want {
				t.Errorf("%s: digest %s, want %s", vector.name, vector.digest, want)
			}
		}
	}
	if found != len(rust) {
		t.Errorf("found %d of %d Rust digests", found, len(rust))
	}
}

func relocateRecord(t *testing.T, raw jsonx.Value, root string) jsonx.Value {
	t.Helper()
	if items, ok := raw.Items(); ok {
		moved := make([]jsonx.Value, 0, len(items))
		for _, item := range items {
			moved = append(moved, relocateRecord(t, item, root))
		}
		return jsonx.Array(moved...)
	}
	members, ok := raw.Members()
	if !ok {
		return raw
	}
	if path, isPath := members0String(members, "path"); isPath {
		return jsonx.Object(member("path", jsonx.String(root+"/"+path[strings.LastIndexByte(path, '/')+1:])))
	}
	moved := make([]jsonx.Member, 0, len(members))
	for _, item := range members {
		moved = append(moved, member(item.Key, relocateRecord(t, item.Value, root)))
	}
	record := jsonx.Object(moved...)
	if tag, _ := record.Get("type"); stringOf(tag) != "Receipt" {
		return record
	}
	fields, _ := record.Get("fields")
	resource := must(decodeState(encodeState(must(fieldOf(fields, "resource"))), resourceRefFromState))
	claims, _ := must(fieldOf(fields, "claims")).Items()
	rewritten := make([]jsonx.Value, 0, len(claims))
	for _, owned := range claims {
		ownedFields, _ := owned.Get("fields")
		claim := must(decodeState(encodeState(must(fieldOf(ownedFields, "claim"))), claimFromState))
		identity := must(claim.identity(resource.Destination))
		rewritten = append(rewritten, replaceField(owned, "fields", replaceField(ownedFields, "claim_id", jsonx.String(identity))))
	}
	return replaceField(record, "fields", replaceField(fields, "claims", jsonx.Array(rewritten...)))
}

func fieldOf(value jsonx.Value, key string) (jsonx.Value, error) {
	item, ok := value.Get(key)
	if !ok {
		return jsonx.Value{}, errors.New("missing field " + key)
	}
	return item, nil
}

func replaceField(value jsonx.Value, key string, replacement jsonx.Value) jsonx.Value {
	members, _ := value.Members()
	replaced := make([]jsonx.Member, 0, len(members))
	for _, item := range members {
		if item.Key == key {
			item.Value = replacement
		}
		replaced = append(replaced, item)
	}
	return jsonx.Object(replaced...)
}

func TestConfigurationRecordsRelocateToTheExtract(t *testing.T) {
	f := configurations(t)
	extract := map[string][]byte{}
	for _, vector := range stateVectors(t) {
		extract[vector.name] = vector.data
	}
	for _, kind := range []string{"receipts", "journals", "preparations"} {
		for _, item := range f.cases(t, kind) {
			name := "fixture-" + strings.TrimSuffix(kind, "s") + "-" + text(t, item, "name")
			record, _ := item.Get("record")
			moved := encodeState(relocateRecord(t, record, "/pin"))
			if !bytes.Equal(moved, extract[name]) {
				t.Errorf("%s: relocated record\n%s\nwant\n%s", name, moved, extract[name])
			}
		}
	}
}

func TestFixturePreparationNamesItsTransactionPath(t *testing.T) {
	for _, vector := range stateVectors(t) {
		if vector.name != "fixture-preparation-create-section" {
			continue
		}
		value := must(decodeState(vector.data, preparationFromState))
		if want := must(TransactionPath(value.resource.Destination, value.transactionID)); value.directory != want {
			t.Errorf("directory = %s, want %s", value.directory, want)
		}
	}
}

func TestDecodeAcceptsAnyObjectKeyOrder(t *testing.T) {
	sorted := must(decodeState([]byte(`{"type":"Claim","fields":{"kind":{"enum":"Ownership","value":"file"},"selector":null}}`), claimFromState))
	unsorted := must(decodeState([]byte(`{"type":"Claim","fields":{"selector":null,"kind":{"enum":"Ownership","value":"file"}}}`), claimFromState))
	if !sorted.Equal(unsorted) || sorted.Kind() != OwnershipFile {
		t.Errorf("decoded %#v and %#v", sorted, unsorted)
	}
}

func TestCodecRoundTripsNonASCIIAndControlEscapes(t *testing.T) {
	member := must(NewMember(must(NewString("café \x1f 日本語")), []string{"café-key"}))
	claim := must(NewClaim(OwnershipStructured, must(NewSelector(member))))
	data := encodeRecord(claim)
	expected := `{"fields":{"kind":{"enum":"Ownership","value":"structured"},` +
		`"selector":{"fields":{"parts":[{"fields":` +
		`{"identity":{"value":["string","café \u001f 日本語"]},` +
		`"key":["café-key"]},"type":"Member"}]},"type":"Selector"}},` +
		`"type":"Claim"}` + "\n"
	if string(data) != expected {
		t.Errorf("encoded\n%s\nwant\n%s", data, expected)
	}
	if decoded := must(decodeState(data, claimFromState)); !decoded.Equal(claim) {
		t.Errorf("round trip = %#v", decoded)
	}
}

func TestDecodeOutcomesMatchThePythonReference(t *testing.T) {
	cases := []struct{ kind, record, expected string }{
		{"Ancestor", `{"fields":{"device":0,"gid":0,"inode":0,"mode":4095,"path":{"path":"/a"},"security":{"bytes":""},"uid":0},"type":"Ancestor"}`, ""},
		{"Ancestor", `{"fields":{"device":0,"gid":0,"inode":1000000000000000000000000000000,"mode":0,"path":{"path":"/a"},"security":{"bytes":""},"uid":0},"type":"Ancestor"}`, ""},
		{"Node", `{"fields":{"data":{"bytes":"78"},"device":0,"gid":0,"inode":0,"mode":511,"path":"","security":{"bytes":""},"uid":0},"type":"Node"}`, ""},
		{"Node", `{"fields":{"data":{"bytes":"78"},"device":0,"gid":0,"inode":1000000000000000000000000000000,"mode":0,"path":"","security":{"bytes":""},"uid":0},"type":"Node"}`, ""},
		{"Key", "\xef\xbb\xbf" + `{"fields":{"name":"a"},"type":"Key"}`, ""},
		{"Key", `{"fields":{"name":5},"type":"Key"}`, "invalid state record"},
		{"Key", `{"fields":{"name":1.5},"type":"Key"}`, "invalid state scalar"},
		{"Member", `{"fields":{"identity":{"value":["string","x"]},"key":"ab"},"type":"Member"}`, "invalid state record"},
		{"Member", `{"fields":{"identity":"x","key":[]},"type":"Member"}`, "invalid state record"},
		{"DocumentSchema", `{"fields":{"format":"json","key":"k","value":{"value":["string","v"]}},"type":"DocumentSchema"}`, "document schema scaffolds require JSON or JSONC"},
		{"DocumentSchema", `{"fields":{"format":{"enum":"DocumentFormat","value":"json"},"key":"k","value":{"bytes":"61"}},"type":"DocumentSchema"}`, "invalid state record"},
		{"ArrayPosition", `{"fields":{"following":null,"prefix":5,"previous":null},"type":"ArrayPosition"}`, "invalid state record"},
		{"SelectionSnapshot", `{"fields":{"layout":{"bytes":""},"owned_syntax":{"bytes":""},"position":5,"syntax":{"bytes":""},"value":{"bytes":"78"}},"type":"SelectionSnapshot"}`, "invalid state record"},
		{"SelectionSnapshot", `{"fields":{"layout":{"bytes":""},"owned_syntax":{"bytes":""},"position":null,"syntax":{"bytes":""},"value":5},"type":"SelectionSnapshot"}`, "invalid state record"},
		{"DocumentProvenance", `{"fields":{"containers":[],"empty_document":5,"schema_empty_document":null,"schema_fields":[],"trailing_separators":[]},"type":"DocumentProvenance"}`, "invalid state record"},
		{"DocumentProvenance", `{"fields":{"containers":[],"empty_document":null,"schema_empty_document":5,"schema_fields":[],"trailing_separators":[]},"type":"DocumentProvenance"}`, "schema empty-document evidence must be whitespace bytes"},
		{"DocumentProvenance", `{"fields":{"containers":[],"empty_document":null,"schema_empty_document":null,"schema_fields":[],"trailing_separators":[5]},"type":"DocumentProvenance"}`, "separator evidence must be SHA-256 digests"},
		{"CreatedContainer", `{"fields":{"empty_syntax":5,"selector":{"fields":{"parts":[{"fields":{"name":"a"},"type":"Key"}]},"type":"Selector"}},"type":"CreatedContainer"}`, "invalid state record"},
		{"Ancestor", `{"fields":{"device":0,"gid":0,"inode":0,"mode":0,"path":"/a","security":{"bytes":""},"uid":0},"type":"Ancestor"}`, "ancestor path must be absolute"},
		{"Ancestor", `{"fields":{"device":"0","gid":0,"inode":0,"mode":0,"path":{"path":"/a"},"security":{"bytes":""},"uid":0},"type":"Ancestor"}`, "invalid ancestor metadata"},
		{"Ancestor", `{"fields":{"device":true,"gid":0,"inode":0,"mode":0,"path":{"path":"/a"},"security":{"bytes":""},"uid":0},"type":"Ancestor"}`, "invalid ancestor metadata"},
		{"Ancestor", `{"fields":{"device":0,"gid":0,"inode":0,"mode":4096,"path":{"path":"/a"},"security":{"bytes":""},"uid":0},"type":"Ancestor"}`, "invalid ancestor metadata"},
		{"Ancestor", `{"fields":{"device":0,"gid":0,"inode":-1000000000000000000000000000000,"mode":0,"path":{"path":"/a"},"security":{"bytes":""},"uid":0},"type":"Ancestor"}`, "invalid ancestor metadata"},
		{"Ancestor", `{"fields":{"device":0,"gid":0,"inode":0,"mode":0,"path":{"path":"/a"},"security":5,"uid":0},"type":"Ancestor"}`, "invalid state record"},
		{"Ancestor", `{"fields":{"device":0,"gid":0,"inode":0,"mode":0,"path":{"path":"/a"},"security":{"bytes":"abc"},"uid":0},"type":"Ancestor"}`, "non-hexadecimal number found in fromhex() arg at position 3"},
		{"Ancestor", `{"fields":{"device":0,"gid":0,"inode":0,"mode":0,"path":{"path":"/a"},"security":{"bytes":"zz"},"uid":0},"type":"Ancestor"}`, "non-hexadecimal number found in fromhex() arg at position 0"},
		{"Ancestor", `{"fields":{"device":0,"gid":0,"inode":0,"mode":0,"path":{"path":"/a"},"security":{"bytes":"ab0g"},"uid":0},"type":"Ancestor"}`, "non-hexadecimal number found in fromhex() arg at position 3"},
		{"Ancestor", `{"fields":{"device":0,"gid":0,"inode":0,"mode":0,"path":{"path":"/a"},"security":{"bytes":"ab cd"},"uid":0},"type":"Ancestor"}`, "state record is not canonical"},
		{"Ancestor", `{"fields":{"device":0,"gid":0,"inode":0,"mode":0,"path":{"path":"/a"},"uid":0},"type":"Ancestor"}`, "state record fields do not match schema"},
		{"Node", `{"fields":{"data":{"bytes":"78"},"device":0,"gid":0,"inode":0,"mode":0,"path":5,"security":{"bytes":""},"uid":0},"type":"Node"}`, "invalid state record"},
		{"Node", `{"fields":{"data":"x","device":0,"gid":0,"inode":0,"mode":0,"path":"","security":{"bytes":""},"uid":0},"type":"Node"}`, "invalid state record"},
		{"Node", `{"fields":{"data":{"bytes":"78"},"device":0,"gid":0,"inode":0,"mode":true,"path":"","security":{"bytes":""},"uid":0},"type":"Node"}`, "invalid revision metadata"},
		{"Node", `{"fields":{"data":{"bytes":"78"},"device":0,"gid":0,"inode":0,"mode":512,"path":"","security":{"bytes":""},"uid":0},"type":"Node"}`, "invalid revision metadata"},
		{"Node", `{"fields":{"data":{"bytes":"78"},"device":0,"gid":0,"inode":-1000000000000000000000000000000,"mode":0,"path":"","security":{"bytes":""},"uid":0},"type":"Node"}`, "invalid revision metadata"},
		{"Revision", `{"fields":{"nodes":[{"fields":{"name":"a"},"type":"Key"}]},"type":"Revision"}`, "invalid state record"},
		{"Claim", `{"fields":{"kind":{"enum":"Ownership","value":5},"selector":null},"type":"Claim"}`, "5 is not a valid Ownership"},
		{"Claim", `{"fields":{"kind":{"enum":"Ownership","value":true},"selector":null},"type":"Claim"}`, "True is not a valid Ownership"},
		{"Claim", `{"fields":{"kind":{"enum":"Ownership","value":null},"selector":null},"type":"Claim"}`, "None is not a valid Ownership"},
		{"Claim", `{"fields":{"kind":{"enum":"Ownership","value":{}},"selector":null},"type":"Claim"}`, "{} is not a valid Ownership"},
		{"Claim", `{"fields":{"kind":{"enum":"Ownership","value":[]},"selector":null},"type":"Claim"}`, "[] is not a valid Ownership"},
		{"Claim", `{"fields":{"kind":{"enum":"Ownership","value":1.5},"selector":null},"type":"Claim"}`, "1.5 is not a valid Ownership"},
		{"Claim", `{"fields":{"kind":{"enum":"Ownership","value":"bogus"},"selector":null},"type":"Claim"}`, "'bogus' is not a valid Ownership"},
		{"Claim", `{"fields":{"kind":"file","selector":null},"type":"Claim"}`, "invalid state record"},
		{"Claim", `{"fields":{"kind":{"enum":"Disposition","value":"created"},"selector":null},"type":"Claim"}`, "invalid state record"},
		{"Claim", `{"fields":{"kind":{"enum":"Ownership","value":"file"},"selector":5},"type":"Claim"}`, "whole-resource claims cannot have selectors"},
		{"Claim", `{"fields":{"kind":{"enum":"Ownership","value":"file"},"selector":{"fields":{"parts":[{"fields":{"name":"a"},"type":"Key"}]},"type":"Selector"}},"type":"Claim"}`, "whole-resource claims cannot have selectors"},
		{"Claim", `{"fields":{"kind":{"enum":"Ownership","value":"structured"},"selector":5},"type":"Claim"}`, "invalid state record"},
		{"ResourceRef", `{"fields":{"destination":{"path":"/pin/AGENTS.md"},"kind":{"enum":"ResourceKind","value":"document"},"schema_version":3},"type":"ResourceRef"}`, "authority schema must be 2"},
		{"ResourceRef", `{"fields":{"destination":{"path":"/pin/AGENTS.md"},"kind":{"enum":"ResourceKind","value":"document"},"schema_version":true},"type":"ResourceRef"}`, "authority schema must be 2"},
		{"ResourceRef", `{"fields":{"destination":{"path":"/pin/AGENTS.md"},"kind":"document","schema_version":2},"type":"ResourceRef"}`, "invalid state record"},
		{"ResourceRef", `{"fields":{"destination":"/pin/AGENTS.md","kind":{"enum":"ResourceKind","value":"document"},"schema_version":2},"type":"ResourceRef"}`, "expected an absolute concrete path"},
		{"ResourceRef", `{"fields":{"destination":{"path":"/pin/AGENTS.md"},"kind":{"enum":"ResourceKind","value":"document"},"schema_version":2,"state_root":{"path":"/pin/x"}},"type":"ResourceRef"}`, "state record fields do not match schema"},
		{"Preparation", `{"fields":{"directory":{"path":"/pin/x"},"resource":{"fields":{"destination":{"path":"/pin/AGENTS.md"},"kind":{"enum":"ResourceKind","value":"document"},"schema_version":2},"type":"ResourceRef"},"schema_version":2,"transaction_id":"1111"},"type":"Preparation"}`, "invalid hexadecimal identity"},
		{"Preparation", `{"fields":{"directory":{"path":"x"},"resource":{"fields":{"destination":{"path":"/pin/AGENTS.md"},"kind":{"enum":"ResourceKind","value":"document"},"schema_version":2},"type":"ResourceRef"},"schema_version":2,"transaction_id":"11111111111111111111111111111111"},"type":"Preparation"}`, "state paths must be absolute without traversal"},
		{"Preparation", `{"fields":{"directory":{"path":"/pin/x"},"resource":5,"schema_version":2,"transaction_id":"11111111111111111111111111111111"},"type":"Preparation"}`, "invalid state record"},
		{"Preparation", `{"fields":{"directory":{"path":"/pin/x"},"resource":{"fields":{"destination":{"path":"/pin/AGENTS.md"},"kind":{"enum":"ResourceKind","value":"document"},"schema_version":2},"type":"ResourceRef"},"schema_version":3,"transaction_id":"11111111111111111111111111111111"},"type":"Preparation"}`, "preparation schema must be 2"},
		{"Preparation", `{"fields":{"directory":{"path":"/pin/x"},"resource":{"fields":{"destination":{"path":"/pin/AGENTS.md"},"kind":{"enum":"ResourceKind","value":"document"},"schema_version":2},"type":"ResourceRef"},"schema_version":2,"transaction_id":"11111111111111111111111111111111"},"type":"Preparation"}`, ""},
		{"Index", `{"fields":{"bundle_id":5,"context_digest":"0000000000000000000000000000000000000000000000000000000000000000","current":null,"pending":null,"previous":null,"residual":[],"schema_version":2},"type":"Index"}`, "invalid state record"},
		{"Index", `{"fields":{"bundle_id":"owner","context_digest":"0000000000000000000000000000000000000000000000000000000000000000","current":5,"pending":null,"previous":null,"residual":[],"schema_version":2},"type":"Index"}`, "invalid state record"},
		{"Index", `{"fields":{"bundle_id":"owner","context_digest":"0000000000000000000000000000000000000000000000000000000000000000","current":null,"pending":null,"previous":null,"residual":[],"schema_version":3},"type":"Index"}`, "index schema must be 2"},
		{"Node", `{"fields":{"data":{"bytes":"78"},"device":0,"gid":0,"inode":0,"mode":NaN,"path":"","security":{"bytes":""},"uid":0},"type":"Node"}`, "non-JSON numeric constant: NaN"},
		{"Node", `{"fields":{"data":{"bytes":"78"},"device":0,"gid":0,"inode":0,"mode":Infinity,"path":"","security":{"bytes":""},"uid":0},"type":"Node"}`, "non-JSON numeric constant: Infinity"},
		{"Node", `{"fields":{"data":{"bytes":"78"},"device":0,"gid":0,"inode":0,"mode":-Infinity,"path":"","security":{"bytes":""},"uid":0},"type":"Node"}`, "non-JSON numeric constant: -Infinity"},
		{"Key", `{"fields":{"name":"a"},"type":"Key","type":"Key"}`, "duplicate JSON field: type"},
		{"Key", `{"fields":{"name":"\ud800"},"type":"Key"}`, "JSON escape contains an unpaired surrogate"},
		{"Claim", `{"type":"NotAThing","fields":{}}`, "unknown state record"},
		{"Claim", `{"type":"Claim","fields":{"kind":{"enum":"Ownership","value":"file"},"selector":null,"extra":1}}`, "state record fields do not match schema"},
		{"Claim", `{"type":"Claim","fields":{"kind":{"enum":"Ownership","value":"file"},"selector":1.5}}`, "invalid state scalar"},
		{"Ancestor", `{"type":"Ancestor","fields":{"path":{"path":"/a/../b"},"device":0,"inode":0,"mode":0,"uid":0,"gid":0,"security":{"bytes":""}}}`, "state paths must be absolute without traversal"},
		{"Ancestor", `{"type":"Ancestor","fields":{"path":{"path":"relative"},"device":0,"inode":0,"mode":0,"uid":0,"gid":0,"security":{"bytes":""}}}`, "state paths must be absolute without traversal"},
		{"ArrayPosition", `{"type":"ArrayPosition","fields":{"previous":null,"following":null,"prefix":{"bytes":"AB"}}}`, "state record is not canonical"},
		{"ArrayPosition", `{"type":"ArrayPosition","fields":{"previous":null,"following":null,"prefix":{"bytes":"ab"}}}`, ""},
		{"DocumentProvenance", `{"fields":{"containers":[],"empty_document":null,"schema_empty_document":null,"schema_fields":[],"trailing_separators":"x"},"type":"DocumentProvenance"}`, "invalid state record"},
		{"DocumentProvenance", `{"fields":{"containers":[],"empty_document":null,"schema_empty_document":null,"schema_fields":"x","trailing_separators":[]},"type":"DocumentProvenance"}`, "invalid state record"},
		{"DocumentProvenance", `{"fields":{"containers":"x","empty_document":null,"schema_empty_document":null,"schema_fields":[],"trailing_separators":[]},"type":"DocumentProvenance"}`, "invalid state record"},
		{"DocumentProvenance", `{"fields":{"containers":[],"empty_document":null,"schema_empty_document":null,"schema_fields":[],"trailing_separators":""},"type":"DocumentProvenance"}`, "invalid state record"},
		{"Selector", `{"fields":{"parts":"x"},"type":"Selector"}`, "invalid state record"},
		{"Key", "{\"fields\":{\"name\":\"\xff\"},\"type\":\"Key\"}", "invalid state record"},
		{"Key", `{"fields":{"name":"a"},"type":["Key"]}`, "invalid state record"},
		{"Claim", `{"fields":{"kind":{"enum":"Ownership","value":"section"},"selector":"guide"},"type":"Claim"}`, "state record is not canonical"},
	}
	for _, c := range cases {
		if got := decodeOutcome(c.kind, []byte(c.record)); got != c.expected {
			t.Errorf("%s record %s: got %q, want %q", c.kind, c.record, got, c.expected)
		}
	}
}

func TestDecodeRejectsOversizedNesting(t *testing.T) {
	deep := strings.Repeat("[", 600) + strings.Repeat("]", 600)
	record := `{"fields":{"name":` + deep + `},"type":"Key"}`
	if got := decodeOutcome("Key", []byte(record)); got != "JSON nesting is too deep" {
		t.Errorf("deep record: got %q", got)
	}
	shallow := strings.Repeat("[", 500) + strings.Repeat("]", 500)
	record = `{"fields":{"name":` + shallow + `},"type":"Key"}`
	if got := decodeOutcome("Key", []byte(record)); got != "invalid state record" {
		t.Errorf("nested name: got %q", got)
	}
}

func TestDecodePairOutcomesMatchThePythonReference(t *testing.T) {
	reference := `{"fields":{"destination":{"path":"/pin/AGENTS.md"},"kind":{"enum":"ResourceKind","value":"document"},"schema_version":2},"type":"ResourceRef"}`
	digest := strings.Repeat("0", 64)
	cases := []struct{ template, expected string }{
		{`{"fields":{"bundle_digest":"<digest>","membership":[[<reference>,["<digest>"]]],"render_digest":"<digest>","version":5},"type":"Generation"}`, "invalid state record"},
		{`{"fields":{"bundle_digest":5,"membership":[[<reference>,["<digest>"]]],"render_digest":"<digest>","version":"1.0"},"type":"Generation"}`, "invalid hexadecimal identity"},
		{`{"fields":{"bundle_digest":"<digest>","membership":[[<reference>]],"render_digest":"<digest>","version":"1.0"},"type":"Generation"}`, "not enough values to unpack (expected 2, got 1)"},
		{`{"fields":{"bundle_digest":"<digest>","membership":[[<reference>,["<digest>"],"extra"]],"render_digest":"<digest>","version":"1.0"},"type":"Generation"}`, "too many values to unpack (expected 2)"},
		{`{"fields":{"bundle_digest":"<digest>","membership":[[<reference>,[5]]],"render_digest":"<digest>","version":"1.0"},"type":"Generation"}`, "invalid hexadecimal identity"},
		{`{"fields":{"bundle_digest":"<digest>","membership":[[<reference>,["<digest>"]]],"render_digest":"<digest>","version":"1.0"},"type":"Generation"}`, ""},
		{`{"fields":{"bundle_digest":"<digest>","membership":[[<reference>,["<digest>","<digest>"]]],"render_digest":"<digest>","version":"1.0"},"type":"Generation"}`, "duplicate membership claim"},
		{`{"fields":{"bundle_digest":"<digest>","membership":[[<reference>,[]],[<reference>,[]]],"render_digest":"<digest>","version":"1.0"},"type":"Generation"}`, "duplicate generation resource"},
	}
	for _, c := range cases {
		record := strings.ReplaceAll(strings.ReplaceAll(c.template, "<reference>", reference), "<digest>", digest)
		if got := decodeOutcome("Generation", []byte(record)); got != c.expected {
			t.Errorf("generation record %s: got %q, want %q", record, got, c.expected)
		}
	}
}

func TestDecodeMetadataReportsTheWrongTypeAfterCanonicalCheck(t *testing.T) {
	recordIndex := must(newIndex("team", strings.Repeat("a", 64), nil, nil, nil, nil, 2))
	data := encodeRecord(recordIndex)
	if value, err := decodeMetadata(data, "Index", indexFromState); err != nil || value.bundleID != "team" {
		t.Errorf("decodeMetadata(Index) = %v, %v", value, err)
	}
	_, err := decodeMetadata(data, "Receipt", receiptFromState)
	if err == nil || err.Error() != "metadata record has the wrong type" {
		t.Errorf("wrong type error = %v", err)
	}
	spaced := append([]byte{data[0], ' '}, data[1:]...)
	if _, err := decodeMetadata(spaced, "Index", indexFromState); err != nil {
		t.Errorf("whitespace is not part of the canonical tree: %v", err)
	}
	upper := []byte(`{"fields":{"following":null,"prefix":{"bytes":"AB"},"previous":null},"type":"ArrayPosition"}`)
	_, err = decodeMetadata(upper, "Index", indexFromState)
	if err == nil || err.Error() != "state record is not canonical" {
		t.Errorf("non-canonical wrong type error = %v", err)
	}
	_, err = decodeMetadata([]byte(`{"fields":{"name":1.5},"type":"Key"}`), "Index", indexFromState)
	if err == nil || err.Error() != "invalid state scalar" {
		t.Errorf("float wrong type error = %v", err)
	}
}

func TestStateRecordsValidateLikeRust(t *testing.T) {
	small := func(value int64) count { return count{small: value} }
	file := must(newNode("", []byte("x"), true, fileIdentity{mode: small(0o644), uid: small(0), gid: small(0), device: small(1), inode: small(1)}, nil))
	dir := must(newNode("", nil, false, fileIdentity{mode: small(0o755), uid: small(0), gid: small(0), device: small(1), inode: small(1)}, nil))
	child := must(newNode("a", []byte("y"), true, fileIdentity{mode: small(0o644), uid: small(0), gid: small(0), device: small(1), inode: small(2)}, nil))
	cases := []struct {
		name  string
		err   error
		value string
	}{
		{"duplicate root", errorOf(newRevision([]node{dir, dir})), "revision must have exactly one root"},
		{"child first", errorOf(newRevision([]node{child})), "revision must have exactly one root"},
		{"file parent", errorOf(newRevision([]node{file, child})), "revision parents must be directories"},
		{"unordered", errorOf(newRevision([]node{dir, must(newNode("b", nil, false, fileIdentity{mode: small(0o755), uid: small(0), gid: small(0), device: small(1), inode: small(3)}, nil)), child})), "revision entries must be ordered"},
		{"root resource", errorOf(newResourceRef("/", ResourceDocument, 2)), "filesystem roots are not resources"},
		{"traversal resource", errorOf(newResourceRef("/a/../b", ResourceDocument, 2)), "expected an absolute concrete path"},
		{"node path", errorOf(newNode("a/../b", nil, false, fileIdentity{mode: small(0), uid: small(0), gid: small(0), device: small(0), inode: small(0)}, nil)), "revision path must be an unambiguous relative path"},
		{"generation version", errorOf(newGeneration(" ", strings.Repeat("a", 64), strings.Repeat("a", 64), nil)), "version must be nonblank"},
		{"index owner", errorOf(newIndex("Team", strings.Repeat("a", 64), nil, nil, nil, nil, 2)), "index owner must be 1-64 lowercase ASCII letters/digits with internal hyphens"},
	}
	for _, c := range cases {
		if c.err == nil || c.err.Error() != c.value {
			t.Errorf("%s: error = %v, want %q", c.name, c.err, c.value)
		}
	}
	if _, err := newRevision([]node{dir, child}); err != nil {
		t.Errorf("valid revision: %v", err)
	}
	if data, ok := must(newRevision([]node{file})).data(); !ok || string(data) != "x" {
		t.Errorf("file revision data = %q, %v", data, ok)
	}
}

func TestIndexResourcesConflictOnSharedDestinations(t *testing.T) {
	digest := strings.Repeat("a", 64)
	document := must(newResourceRef("/pin/a", ResourceDocument, 2))
	tree := must(newResourceRef("/pin/a", ResourceTree, 2))
	current := must(newGeneration("1", digest, digest, []membership{{resource: tree}}))
	_, err := newIndex("team", digest, &current, nil, nil, []ResourceRef{document}, 2)
	if err == nil || err.Error() != "index has conflicting resource kinds" {
		t.Errorf("conflicting index error = %v", err)
	}
	other := must(newResourceRef("/pin/0", ResourceDocument, 2))
	shared := must(newGeneration("1", digest, digest, []membership{{resource: document}, {resource: other}}))
	value := must(newIndex("team", digest, &shared, &shared, nil, []ResourceRef{document}, 2))
	if got := value.resources(); len(got) != 2 || got[0] != other || got[1] != document {
		t.Errorf("resources = %v", got)
	}
}

func TestFileIdentityFieldsDecodeByName(t *testing.T) {
	want := fileIdentity{device: count{small: 11}, inode: count{small: 12}, mode: count{small: 0o640}, uid: count{small: 14}, gid: count{small: 15}}
	decodedNode := must(decodeState([]byte(`{"type":"Node","fields":{"data":null,"device":11,"gid":15,"inode":12,"mode":416,`+
		`"path":"","security":{"bytes":""},"uid":14}}`), nodeFromState))
	if decodedNode.fileIdentity != want {
		t.Errorf("node identity = %+v, want %+v", decodedNode.fileIdentity, want)
	}
	decodedAncestor := must(decodeState([]byte(`{"type":"Ancestor","fields":{"device":11,"gid":15,"inode":12,"mode":416,`+
		`"path":{"path":"/fixture"},"security":{"bytes":""},"uid":14}}`), ancestorFromState))
	if decodedAncestor.fileIdentity != want {
		t.Errorf("ancestor identity = %+v, want %+v", decodedAncestor.fileIdentity, want)
	}
	if got := string(encodeRecord(decodedNode)); got != `{"fields":{"data":null,"device":11,"gid":15,"inode":12,"mode":416,`+
		`"path":"","security":{"bytes":""},"uid":14},"type":"Node"}`+"\n" {
		t.Errorf("node encodes as %s", got)
	}
}

func TestCountsKeepExactUnsignedValues(t *testing.T) {
	cases := []struct {
		value count
		want  uint64
		ok    bool
	}{
		{count{small: 7}, 7, true},
		{count{big: "18446744073709551615"}, 18446744073709551615, true},
		{count{big: "18446744073709551616"}, 0, false},
		{count{small: -1}, 0, false},
	}
	for _, c := range cases {
		if got, ok := c.value.uint64(); ok != c.ok || ok && got != c.want {
			t.Errorf("%#v.uint64() = %d, %v", c.value, got, ok)
		}
	}
	if got := string(jsonx.Encode(count{big: "-1000000000000000000000"}.json())); got != "-1000000000000000000000" {
		t.Errorf("big count encodes as %s", got)
	}
}

func TestReceiptComparisonIgnoresOnlyTheTransaction(t *testing.T) {
	for _, vector := range stateVectors(t) {
		if vector.typeName != "Receipt" {
			continue
		}
		value := must(decodeState(vector.data, receiptFromState))
		moved := value
		moved.transactionID = strings.Repeat("f", 32)
		if !value.equivalentExceptTransaction(moved) {
			t.Errorf("%s: transaction change breaks equivalence", vector.name)
		}
		changed := value
		changed.provenance = DocumentProvenance{hasEmptyDocument: true}
		if value.equivalentExceptTransaction(changed) {
			t.Errorf("%s: provenance change keeps equivalence", vector.name)
		}
	}
}

func FuzzDecodeState(f *testing.F) {
	for _, vector := range stateVectors(f) {
		f.Add(vector.typeName, vector.data)
	}
	f.Fuzz(func(t *testing.T, typeName string, data []byte) {
		if !slices.Contains(stateTypeNames, typeName) {
			typeName = "Receipt"
		}
		encoded, err := decodeAs(typeName, data)
		if err != nil {
			return
		}
		again, err := decodeAs(typeName, encoded)
		if err != nil {
			t.Fatalf("re-decode of %s failed: %v", encoded, err)
		}
		if !bytes.Equal(again, encoded) {
			t.Fatalf("re-encode changed %s to %s", encoded, again)
		}
	})
}

func TestPathsWithNULComponentsStayDistinct(t *testing.T) {
	digest := strings.Repeat("a", 64)
	joined := "/pin/a\x00b"
	nested := "/pin/a/b"
	if pathKey(joined) == pathKey(nested) {
		t.Fatalf("pathKey collides for %q and %q", joined, nested)
	}
	if err := checkRequirements(ownedClaim{requirements: []requirement{{joined, digest}, {nested, digest}}}); err != nil {
		t.Errorf("requirements: %v", err)
	}
	first := must(newResourceRef(joined, ResourceDocument, 2))
	second := must(newResourceRef(nested, ResourceDocument, 2))
	generation, err := newGeneration("1", digest, digest, []membership{{resource: first}, {resource: second}})
	if err != nil {
		t.Errorf("generation: %v", err)
	}
	if _, err := newIndex("team", digest, &generation, nil, nil, []ResourceRef{first, second}, 2); err != nil {
		t.Errorf("index: %v", err)
	}
}
