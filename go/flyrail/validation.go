package flyrail

import (
	"slices"
	"strings"
	"unicode/utf8"

	"github.com/nielsmadan/flyrail/go/internal/unicodex"
)

var reservedNames = []string{
	"con", "prn", "aux", "nul", "conin$", "conout$", "com1", "com2", "com3", "com4", "com5",
	"com6", "com7", "com8", "com9", "com¹", "com²", "com³", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5",
	"lpt6", "lpt7", "lpt8", "lpt9", "lpt¹", "lpt²", "lpt³",
}

func validateIdentifier(value, label string) error {
	length := utf8.RuneCountInString(value)
	shaped := length >= 1 && length <= 64
	for segment := range strings.SplitSeq(value, "-") {
		shaped = shaped && segment != "" && strings.IndexFunc(segment, func(r rune) bool {
			return (r < 'a' || r > 'z') && (r < '0' || r > '9')
		}) < 0
	}
	if !shaped {
		return newValueError(label + " must be 1-64 lowercase ASCII letters/digits with internal hyphens")
	}
	if slices.Contains(reservedNames, value) {
		return newValueError(label + " is a reserved Windows name")
	}
	return nil
}

func validateVersion(value string) error {
	if unicodex.IsPythonBlank(value) {
		return newValueError("version must be nonblank")
	}
	return nil
}

func isControl(r rune) bool {
	return r <= 0x1F || (r >= 0x7F && r <= 0x9F)
}

func validateRelativePath(value, label string) error {
	parts := strings.Split(value, "/")
	if value == "" || slices.ContainsFunc(parts, func(part string) bool { return part == "" || part == "." || part == ".." }) {
		return newValueError(label + " must be an unambiguous relative path")
	}
	if !utf8.ValidString(value) || strings.ContainsFunc(value, func(r rune) bool {
		return strings.ContainsRune(`<>:"\|?*`, r) || isControl(r)
	}) {
		return newValueError(label + " contains a nonportable character")
	}
	if !unicodex.IsNFC(value) {
		return newValueError(label + " must contain NFC-normalized Unicode scalar values")
	}
	for _, part := range parts {
		if strings.HasSuffix(part, ".") || strings.HasSuffix(part, " ") {
			return newValueError(label + " contains a component ending in a dot or space")
		}
		stem, _, _ := strings.Cut(part, ".")
		if slices.Contains(reservedNames, unicodex.Fold(strings.TrimRight(stem, " "))) {
			return newValueError(label + " contains a reserved Windows name")
		}
	}
	return nil
}

func portablePathKey(value string) string {
	return unicodex.PortableKey(value)
}

func ancestors(value string) []string {
	parts := strings.Split(value, "/")
	paths := make([]string, 0, len(parts))
	for index := range parts {
		paths = append(paths, strings.Join(parts[:index+1], "/"))
	}
	return paths
}
