package unicodex

import (
	"bufio"
	"bytes"
	"flag"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
	"unicode/utf8"

	"golang.org/x/text/unicode/norm"
)

var reference = flag.String("reference", "", "file of Python str.casefold results: hex code point, tab, hex code points")

func TestNormalizationTablesAreUnicode17(t *testing.T) {
	if norm.Version != "17.0.0" {
		t.Fatalf("norm.Version = %s, want 17.0.0 to match Rust unicode-normalization 0.1.25", norm.Version)
	}
}

func TestSnapshotMatchesTheSwiftCopy(t *testing.T) {
	local, err := os.ReadFile(filepath.Join("testdata", "CaseFolding-16.0.0.txt"))
	if err != nil {
		t.Fatal(err)
	}
	swift, err := os.ReadFile(filepath.Join("..", "..", "..", "swift", "scripts", "unicode", "CaseFolding-16.0.0.txt"))
	if os.IsNotExist(err) && os.Getenv("FLYRAIL_REQUIRE_FIXTURES") != "1" {
		t.Skip("swift/scripts/unicode is not available")
	}
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(local, swift) {
		t.Fatal("testdata/CaseFolding-16.0.0.txt differs from swift/scripts/unicode/CaseFolding-16.0.0.txt")
	}
	if CaseFoldingVersion != "16.0.0" {
		t.Fatalf("CaseFoldingVersion = %s", CaseFoldingVersion)
	}
}

func TestPortableKeyFoldsLikePythonCasefold(t *testing.T) {
	cases := map[string]string{
		"Skills/Notes.MD": "skills/notes.md",
		"STRASSE":         "strasse",
		"straße":          "strasse",
		"ΣΊΣΥΦΟΣ":         "σίσυφοσ",
		"ﬁle":             "file",
		"cafe\u0301":      "café",
		"İ":               "i\u0307",
	}
	for input, expected := range cases {
		if got := PortableKey(input); got != expected {
			t.Errorf("PortableKey(%q) = %q, want %q", input, got, expected)
		}
	}
}

func TestCaseFoldMatchesReference(t *testing.T) {
	if *reference == "" {
		t.Skip("no -reference file; scripts/check_case_folding.py supplies one")
	}
	data, err := os.ReadFile(*reference)
	if err != nil {
		t.Fatal(err)
	}
	expected := map[rune]string{}
	scanner := bufio.NewScanner(bytes.NewReader(data))
	for scanner.Scan() {
		code, mapped, ok := strings.Cut(scanner.Text(), "\t")
		if !ok {
			t.Fatalf("malformed reference line %q", scanner.Text())
		}
		value, err := strconv.ParseUint(code, 16, 32)
		if err != nil {
			t.Fatal(err)
		}
		var folded []rune
		for _, item := range strings.Fields(mapped) {
			unit, err := strconv.ParseUint(item, 16, 32)
			if err != nil {
				t.Fatal(err)
			}
			folded = append(folded, rune(unit))
		}
		expected[rune(value)] = string(folded)
	}
	if err := scanner.Err(); err != nil {
		t.Fatal(err)
	}
	if len(expected) < 1400 {
		t.Fatalf("reference has %d mappings, want the full table", len(expected))
	}
	mismatches := 0
	for r := rune(0); r <= utf8.MaxRune; r++ {
		if r >= 0xD800 && r <= 0xDFFF {
			continue
		}
		want, mapped := expected[r]
		if !mapped {
			want = string(r)
		}
		if got := Fold(string(r)); got != want {
			mismatches++
			if mismatches <= 10 {
				t.Errorf("Fold(U+%04X) = %q, Python casefold = %q", r, got, want)
			}
		}
	}
	if mismatches > 0 {
		t.Fatalf("%d code points differ from Python casefold", mismatches)
	}
}

func TestIsPythonBlank(t *testing.T) {
	for _, value := range []string{"", " ", "\t\n", "\x1c", "\u00a0", "\u2028\u3000"} {
		if !IsPythonBlank(value) {
			t.Errorf("IsPythonBlank(%q) = false", value)
		}
	}
	for _, value := range []string{"1.0.0", "  1 ", "\u200b", "\x00"} {
		if IsPythonBlank(value) {
			t.Errorf("IsPythonBlank(%q) = true", value)
		}
	}
}

func TestLossyReplacesMaximalSubparts(t *testing.T) {
	cases := []struct{ input, expected string }{
		{"plain é", "plain é"},
		{"a\xffb", "a\uFFFDb"},
		{"a\xe2\x82b", "a\uFFFDb"},
		{"\xe2\x82", "\uFFFD"},
		{"\xf0\x9f\x98", "\uFFFD"},
		{"\xed\xa0\x80", "\uFFFD\uFFFD\uFFFD"},
		{"\xc0\xaf", "\uFFFD\uFFFD"},
		{"\xf4\x90\x80\x80", "\uFFFD\uFFFD\uFFFD\uFFFD"},
		{"\xe0\x80\x80", "\uFFFD\uFFFD\uFFFD"},
		{"\xf1\x80\x41", "\uFFFDA"},
		{"\uFFFD", "\uFFFD"},
	}
	for _, c := range cases {
		if got := Lossy(c.input); got != c.expected {
			t.Errorf("Lossy(%q) = %q, want %q", c.input, got, c.expected)
		}
	}
}
