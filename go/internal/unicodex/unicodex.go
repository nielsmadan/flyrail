package unicodex

import (
	"sort"
	"strings"
	"unicode/utf8"

	"golang.org/x/text/unicode/norm"
)

type foldEntry struct {
	code   rune
	folded string
}

// FoldRune returns the full case folding of r, or "" when r folds to itself.
func FoldRune(r rune) string {
	index := sort.Search(len(foldTable), func(i int) bool { return foldTable[i].code >= r })
	if index < len(foldTable) && foldTable[index].code == r {
		return foldTable[index].folded
	}
	return ""
}

// Fold returns the Unicode full case folding of s, as Python's str.casefold.
func Fold(s string) string {
	var out strings.Builder
	out.Grow(len(s))
	for _, r := range s {
		if folded := FoldRune(r); folded != "" {
			out.WriteString(folded)
		} else {
			out.WriteRune(r)
		}
	}
	return out.String()
}

// PortableKey returns the collision key of a portable path: NFC of the full case folding.
func PortableKey(s string) string {
	return norm.NFC.String(Fold(s))
}

// IsNFC reports whether s is in Unicode Normalization Form C.
func IsNFC(s string) bool {
	return norm.NFC.IsNormalString(s)
}

// IsPythonSpace reports whether Python's str.isspace treats r as whitespace.
func IsPythonSpace(r rune) bool {
	switch {
	case r >= '\t' && r <= '\r', r >= 0x1C && r <= ' ':
		return true
	case r == 0x85, r == 0xA0, r == 0x1680, r >= 0x2000 && r <= 0x200A:
		return true
	case r == 0x2028, r == 0x2029, r == 0x202F, r == 0x205F, r == 0x3000:
		return true
	}
	return false
}

// IsPythonBlank reports whether s is empty or consists only of Python whitespace.
func IsPythonBlank(s string) bool {
	for _, r := range s {
		if !IsPythonSpace(r) {
			return false
		}
	}
	return true
}

// Lossy decodes b as UTF-8, replacing each maximal invalid subpart with U+FFFD, as Rust's String::from_utf8_lossy.
func Lossy(b string) string {
	if utf8.ValidString(b) {
		return b
	}
	var out strings.Builder
	for index := 0; index < len(b); {
		if r, size := utf8.DecodeRuneInString(b[index:]); r != utf8.RuneError || size > 1 {
			out.WriteString(b[index : index+size])
			index += size
			continue
		}
		index += invalidSubpartLength(b[index:])
		out.WriteRune(utf8.RuneError)
	}
	return out.String()
}

func invalidSubpartLength(b string) int {
	lead := b[0]
	var width int
	low, high := byte(0x80), byte(0xBF)
	switch {
	case lead >= 0xC2 && lead <= 0xDF:
		width = 2
	case lead == 0xE0:
		width, low = 3, 0xA0
	case lead == 0xED:
		width, high = 3, 0x9F
	case lead >= 0xE1 && lead <= 0xEF:
		width = 3
	case lead == 0xF0:
		width, low = 4, 0x90
	case lead == 0xF4:
		width, high = 4, 0x8F
	case lead >= 0xF1 && lead <= 0xF3:
		width = 4
	default:
		return 1
	}
	length := 1
	for length < width && length < len(b) {
		next := b[length]
		if length == 1 && (next < low || next > high) || length > 1 && (next < 0x80 || next > 0xBF) {
			break
		}
		length++
	}
	return length
}
