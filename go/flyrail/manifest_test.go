package flyrail

import (
	"errors"
	"strings"
	"testing"

	"github.com/google/go-cmp/cmp"
)

func manifestMessage(t *testing.T, raw string) string {
	t.Helper()
	_, _, err := readManifest([]byte(raw))
	failure, ok := errors.AsType[*ValueError](err)
	if !ok {
		t.Fatalf("readManifest(%q) error = %v, want *ValueError", raw, err)
	}
	return failure.Message
}

func TestReadsAValidSchemaOneManifest(t *testing.T) {
	identity, specs, err := readManifest([]byte(`{
		"schema_version": 1,
		"id": "team",
		"version": "1",
		"skills": [{"name": "review", "path": "skills/review", "executables": ["SKILL.md"]}]
	}`))
	if err != nil {
		t.Fatal(err)
	}
	if identity.ID() != "team" || identity.Version() != "1" || len(specs) != 1 {
		t.Fatalf("identity %+v specs %+v", identity, specs)
	}
	if specs[0].Name() != "review" || specs[0].Path() != "skills/review" {
		t.Fatalf("spec = %+v", specs[0])
	}
	if diff := cmp.Diff([]string{"SKILL.md"}, specs[0].Executables()); diff != "" {
		t.Fatalf("executables (-want +got):\n%s", diff)
	}
}

func TestRejectsMalformedManifestDocuments(t *testing.T) {
	cases := map[string]string{
		"not json":                     "",
		"[]":                           "manifest and skill records must be JSON objects",
		"{}":                           "expected fields ['id', 'schema_version', 'skills', 'version']; optional fields []",
		"\xff":                         "manifest is not valid UTF-8",
		"\xef\xbb\xbf{}":               "",
		`{"schema_version":NaN}`:       "non-JSON numeric constant: NaN",
		`{"schema_version":Infinity}`:  "",
		`{"schema_version":-Infinity}`: "",
	}
	for raw, want := range cases {
		if got := manifestMessage(t, raw); want != "" && got != want {
			t.Errorf("%q: %q, want %q", raw, got, want)
		}
	}
}

func TestRejectsNonSchemaOneVersions(t *testing.T) {
	for _, version := range []string{"true", "1.0", `"1"`, "2", "null"} {
		raw := `{"schema_version":` + version + `,"id":"team","version":"1","skills":[]}`
		if got := manifestMessage(t, raw); got != "schema_version must be the integer 1" {
			t.Errorf("%s: %q", version, got)
		}
	}
}

func TestRejectsDuplicateManifestFields(t *testing.T) {
	cases := map[string]string{
		`{"schema_version":1,"id":"team","id":"team","version":"1","skills":[{"name":"review","path":"review"}]}`:     "duplicate JSON field: id",
		`{"schema_version":1,"id":"team","version":"1","skills":[{"name":"review","name":"review","path":"review"}]}`: "duplicate JSON field: name",
		`{"schema_version":1,"id":"team","version":"1","skills":[],"unknown":1}`:                                      "expected fields ['id', 'schema_version', 'skills', 'version']; optional fields []",
		`{"schema_version":1}`: "expected fields ['id', 'schema_version', 'skills', 'version']; optional fields []",
		`{"schema_version":1,"id":"team","version":"1","skills":[{"name":"review","path":"review","extra":null}]}`: "expected fields ['name', 'path']; optional fields ['executables']",
	}
	for raw, want := range cases {
		if got := manifestMessage(t, raw); got != want {
			t.Errorf("%s: %q, want %q", raw, got, want)
		}
	}
}

func TestRejectsInvalidSkillRecords(t *testing.T) {
	prefix := `{"schema_version":1,"id":"team","version":"1","skills":`
	cases := []struct{ skills, want string }{
		{`{}`, "skills must be a nonempty JSON array"},
		{`[]`, "skills must be a nonempty JSON array"},
		{`[null]`, "manifest and skill records must be JSON objects"},
		{`[{"name":"review"}]`, "expected fields ['name', 'path']; optional fields ['executables']"},
		{`[{"name":"review","path":"review","executables":"run.py"}]`, "executables must be a JSON array"},
		{`[{"name":"review","path":"review","executables":[7]}]`, "executable path must be a string"},
		{`[{"name":"review","path":"review","executables":["x","X"]}]`, "executable paths must be unique across portable filesystems"},
		{`[{"name":"review","path":"other"}]`, "skill name must match its containing directory"},
		{`[{"name":"Bad","path":"review"}]`, "skill name must be 1-64 lowercase ASCII letters/digits with internal hyphens"},
	}
	for _, c := range cases {
		if got := manifestMessage(t, prefix+c.skills+"}"); got != c.want {
			t.Errorf("%s: %q, want %q", c.skills, got, c.want)
		}
	}
}

func TestRejectsInvalidIdentityFields(t *testing.T) {
	cases := map[string]string{
		`{"schema_version":1,"id":1,"version":"1","skills":[]}`:          "bundle id must be a string",
		`{"schema_version":1,"id":"Bad","version":"1","skills":[]}`:      "bundle id must be 1-64 lowercase ASCII letters/digits with internal hyphens",
		`{"schema_version":1,"id":"team","version":1,"skills":[]}`:       "version must be a string",
		`{"schema_version":1,"id":"team","version":" \n\t","skills":[]}`: "version must be nonblank",
	}
	for raw, want := range cases {
		if got := manifestMessage(t, raw); got != want {
			t.Errorf("%s: %q, want %q", raw, got, want)
		}
	}
}

func TestRejectsDuplicateAndOverlappingSkillPaths(t *testing.T) {
	prefix := `{"schema_version":1,"id":"x","version":"1","skills":`
	cases := map[string]string{
		`[{"name":"review","path":"one/review"},{"name":"review","path":"two/review"}]`: "duplicate skill name: review",
		`[{"name":"one","path":"one"},{"name":"two","path":"one/two"}]`:                 "skill source paths must not overlap",
		`[{"name":"two","path":"ONE/two"},{"name":"one","path":"one"}]`:                 "skill source paths must not overlap",
	}
	for skills, want := range cases {
		if got := manifestMessage(t, prefix+skills+"}"); got != want {
			t.Errorf("%s: %q, want %q", skills, got, want)
		}
	}
}

func TestKeepsUnicodeVersionsAndRejectsLoneSurrogates(t *testing.T) {
	identity, _, err := readManifest([]byte(`{"schema_version":1,"id":"team","version":"  café 🚀  ","skills":[{"name":"review","path":"review"}]}`))
	if err != nil || identity.Version() != "  café 🚀  " {
		t.Fatalf("identity %+v, %v", identity, err)
	}
	if got := manifestMessage(t, `{"schema_version":1,"id":"team","version":"\ud800","skills":[{"name":"review","path":"review"}]}`); !strings.Contains(got, "surrogate") {
		t.Fatalf("got %q", got)
	}
}

func TestModelsValidateTheirParts(t *testing.T) {
	identity := must(NewBundleIdentity("team", "  older-looking  "))
	if identity.ID() != "team" || identity.Version() != "  older-looking  " {
		t.Fatalf("identity = %+v", identity)
	}
	for _, c := range []struct {
		err  error
		want string
	}{
		{errorOf(NewBundleIdentity("Bad", "1")), "bundle id must be 1-64 lowercase ASCII letters/digits with internal hyphens"},
		{errorOf(NewBundleIdentity("team", " \n\t")), "version must be nonblank"},
		{errorOf(NewBundleIdentity("team", "\xff")), "version must contain Unicode scalar values"},
		{errorOf(NewSkillSpec("review", "skills/other", nil)), "skill name must match its containing directory"},
		{errorOf(NewSkillSpec("review", "review/", nil)), "skill path must be an unambiguous relative path"},
		{errorOf(NewSkillSpec("review", "review", []string{"x", "X"})), "executable paths must be unique across portable filesystems"},
	} {
		failure, ok := errors.AsType[*ValueError](c.err)
		if !ok || failure.Message != c.want {
			t.Errorf("error = %v, want %q", c.err, c.want)
		}
	}
	spec := must(NewSkillSpec("review", "skills/review", []string{"run.py"}))
	if spec.Path() != "skills/review" || spec.Executables()[0] != "run.py" {
		t.Fatalf("spec = %+v", spec)
	}
}
