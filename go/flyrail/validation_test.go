package flyrail

import (
	"strings"
	"testing"

	"github.com/google/go-cmp/cmp"
)

func TestAcceptsHyphenatedLowercaseIdentifiers(t *testing.T) {
	for _, value := range []string{"a", "team-notes", "notes2", "a-b-c", strings.Repeat("x", 64)} {
		if err := validateIdentifier(value, "bundle id"); err != nil {
			t.Errorf("validateIdentifier(%q) = %v", value, err)
		}
	}
}

func TestRejectsMalformedIdentifiers(t *testing.T) {
	for _, value := range []string{"", "-a", "a-", "a--b", "A", "a_b", "a b", "ä", strings.Repeat("x", 65)} {
		err := validateIdentifier(value, "bundle id")
		if err == nil || err.Error() != "bundle id must be 1-64 lowercase ASCII letters/digits with internal hyphens" {
			t.Errorf("validateIdentifier(%q) = %v", value, err)
		}
	}
	for _, value := range []string{"con", "prn", "aux", "nul", "com1", "lpt9"} {
		err := validateIdentifier(value, "bundle id")
		if err == nil || err.Error() != "bundle id is a reserved Windows name" {
			t.Errorf("validateIdentifier(%q) = %v", value, err)
		}
	}
}

func TestRequiresNonblankVersions(t *testing.T) {
	for _, value := range []string{"1.0.0", "  1 "} {
		if err := validateVersion(value); err != nil {
			t.Errorf("validateVersion(%q) = %v", value, err)
		}
	}
	for _, value := range []string{"", " ", "\t\n", "\x1c", "\u00a0"} {
		if err := validateVersion(value); err == nil || err.Error() != "version must be nonblank" {
			t.Errorf("validateVersion(%q) = %v", value, err)
		}
	}
}

func TestAcceptsPortableRelativePaths(t *testing.T) {
	for _, value := range []string{"skills/notes.md", "a/b/c.txt", "café.md", "naïve/file.json"} {
		if err := validateRelativePath(value, "path"); err != nil {
			t.Errorf("validateRelativePath(%q) = %v", value, err)
		}
	}
}

func TestRejectsNonportableRelativePaths(t *testing.T) {
	cases := map[string]string{
		"":                "path must be an unambiguous relative path",
		"/absolute.md":    "path must be an unambiguous relative path",
		"a//b.md":         "path must be an unambiguous relative path",
		"./a.md":          "path must be an unambiguous relative path",
		"../a.md":         "path must be an unambiguous relative path",
		"a/../b.md":       "path must be an unambiguous relative path",
		"a\\b.md":         "path contains a nonportable character",
		"a:b.md":          "path contains a nonportable character",
		"a<b.md":          "path contains a nonportable character",
		"a?b.md":          "path contains a nonportable character",
		"a|b.md":          "path contains a nonportable character",
		"a\"b.md":         "path contains a nonportable character",
		"a*b.md":          "path contains a nonportable character",
		"a\x01b.md":       "path contains a nonportable character",
		"a\x7fb.md":       "path contains a nonportable character",
		"a\u0085b.md":     "path contains a nonportable character",
		"trailing./b.md":  "path contains a component ending in a dot or space",
		"trailing /b.md":  "path contains a component ending in a dot or space",
		"b.md ":           "path contains a component ending in a dot or space",
		"con/file.md":     "path contains a reserved Windows name",
		"a/NUL.txt":       "path contains a reserved Windows name",
		"a/nul .txt":      "path contains a reserved Windows name",
		"a/COM¹.txt":      "path contains a reserved Windows name",
		"a/cafe\u0301.md": "path must contain NFC-normalized Unicode scalar values",
	}
	for value, message := range cases {
		if err := validateRelativePath(value, "path"); err == nil || err.Error() != message {
			t.Errorf("validateRelativePath(%q) = %v, want %q", value, err, message)
		}
	}
}

func TestFoldsCollisionKeysLikePythonCasefold(t *testing.T) {
	cases := map[string]string{
		"Skills/Notes.MD": "skills/notes.md",
		"STRASSE":         "strasse",
		"straße":          "strasse",
		"ΣΊΣΥΦΟΣ":         "σίσυφοσ",
		"ﬁle":             "file",
		"cafe\u0301":      "café",
	}
	for input, expected := range cases {
		if got := portablePathKey(input); got != expected {
			t.Errorf("portablePathKey(%q) = %q, want %q", input, got, expected)
		}
	}
}

func TestAncestorsListEveryPrefix(t *testing.T) {
	if diff := cmp.Diff([]string{"a", "a/b", "a/b/c.md"}, ancestors("a/b/c.md")); diff != "" {
		t.Fatalf("ancestors mismatch (-want +got):\n%s", diff)
	}
}
