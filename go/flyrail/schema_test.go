package flyrail

import (
	"bytes"
	"testing"
)

func versionSchema(t *testing.T, format DocumentFormat, value int64) DocumentSchema {
	t.Helper()
	return must(NewDocumentSchema(format, "version", Int(value)))
}

func TestSchemaMatchingUsesTypedValuesAndJSONCRules(t *testing.T) {
	json := versionSchema(t, FormatJSON, 1)
	cases := []struct {
		data    []byte
		exists  bool
		schema  DocumentSchema
		matches bool
	}{
		{nil, false, json, false},
		{[]byte(" \n"), true, json, false},
		{[]byte("\xef\xbb\xbf{\"version\":1}"), true, json, true},
		{[]byte(`{"version":1.0}`), true, json, false},
		{[]byte(`{/* keep */"version":1}`), true, versionSchema(t, FormatJSONC, 1), true},
	}
	for _, c := range cases {
		if got := must(matchesSchema(c.data, c.exists, c.schema)); got != c.matches {
			t.Errorf("%q: matches %v", c.data, got)
		}
	}
}

func TestSchemaScaffoldRestoresBlankSourceAfterRetirement(t *testing.T) {
	source := []byte(" \r\n")
	installed := must(editSchemaDocument(source, true, nil, nil, DocumentProvenance{}, []DocumentSchema{versionSchema(t, FormatJSON, 1)}, true))
	if !bytes.Equal(installed.After, []byte(`{"version":1}`)) || len(installed.Ownership) != 0 {
		t.Fatalf("install after %q, ownership %d", installed.After, len(installed.Ownership))
	}
	if empty, ok := installed.Provenance.SchemaEmptyDocument(); !ok || !bytes.Equal(empty, source) || len(installed.Provenance.SchemaFields()) != 1 {
		t.Fatalf("install provenance %q %v %d", empty, ok, len(installed.Provenance.SchemaFields()))
	}
	removed := must(editSchemaDocument(installed.After, installed.AfterExists, nil, installed.Ownership, installed.Provenance, nil, true))
	if !bytes.Equal(removed.After, source) || len(removed.Provenance.SchemaFields()) != 0 {
		t.Errorf("remove after %q, fields %d", removed.After, len(removed.Provenance.SchemaFields()))
	}
}

func TestSchemaValidationConflictsWithoutEditing(t *testing.T) {
	data := []byte(`{"version":2}`)
	request := mustSetRequest(t, structuredKeys(t, FormatJSON, Int(1), "x"), AcquireConflict)
	edit := must(editSchemaDocument(data, true, []DocumentRequest{request}, nil, DocumentProvenance{}, []DocumentSchema{versionSchema(t, FormatJSON, 1)}, true))
	if !bytes.Equal(edit.Before, data) || !bytes.Equal(edit.After, data) {
		t.Errorf("conflict changed the document: %q", edit.After)
	}
	if len(edit.Observations) != 1 || edit.Observations[0].Status != EditConflict || edit.Observations[0].Message != "document schema differs" {
		t.Errorf("observations %+v", edit.Observations)
	}
}

func TestSchemaRejectsDuplicateAndAppOwnedKeys(t *testing.T) {
	one, two := versionSchema(t, FormatJSON, 1), versionSchema(t, FormatJSON, 2)
	_, err := editSchemaDocument(nil, false, nil, nil, DocumentProvenance{}, []DocumentSchema{one, two}, true)
	if got := errorText(err); got != "incompatible document schema requirements" {
		t.Errorf("duplicate error %q", got)
	}
	owned := mustSetRequest(t, structuredKeys(t, FormatJSON, Int(1), "version"), AcquireConflict)
	_, err = editSchemaDocument(nil, false, []DocumentRequest{owned}, nil, DocumentProvenance{}, []DocumentSchema{one}, true)
	if got := errorText(err); got != "document schema overlaps an app-owned field" {
		t.Errorf("overlap error %q", got)
	}
	edit := must(editSchemaDocument(nil, false, nil, nil, DocumentProvenance{}, []DocumentSchema{one, one}, true))
	if !bytes.Equal(edit.After, []byte(`{"version":1}`)) {
		t.Errorf("identical requirements after %q", edit.After)
	}
}
