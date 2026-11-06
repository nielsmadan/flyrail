package native

import (
	"strconv"
	"strings"
	"unicode/utf8"

	"github.com/nielsmadan/flyrail/go/internal/pathx"
)

const mountInfoBreaks = "\r\v\f\x1c\x1d\x1e\u0085  "

type mountEntry struct {
	identifier uint64
	point      string
}

// SharedMount reports whether first and second resolve to the same entry of a Linux mountinfo table; doubtful tables share nothing.
func SharedMount(mountInfo []byte, first, second string) bool {
	if !utf8.Valid(mountInfo) {
		return false
	}
	text := string(mountInfo)
	if strings.ContainsAny(text, mountInfoBreaks) {
		return false
	}
	var entries []mountEntry
	for line := range strings.SplitSeq(strings.TrimSuffix(text, "\n"), "\n") {
		if text == "" {
			break
		}
		entry, ok := parseMountLine(line)
		if !ok {
			return false
		}
		entries = append(entries, entry)
	}
	mount, ok := mountID(entries, first)
	if !ok {
		return false
	}
	other, ok := mountID(entries, second)
	return ok && other == mount
}

func parseMountLine(line string) (mountEntry, bool) {
	fields := strings.Split(line, " ")
	if len(fields) < 10 || !digits(fields[0]) || !digits(fields[1]) || !strings.HasPrefix(fields[4], "/") {
		return mountEntry{}, false
	}
	major, minor, found := strings.Cut(fields[2], ":")
	if !found || !digits(major) || !digits(minor) {
		return mountEntry{}, false
	}
	separated := false
	for _, field := range fields[6:] {
		separated = separated || field == "-"
	}
	identifier, err := strconv.ParseUint(fields[0], 10, 64)
	if !separated || err != nil {
		return mountEntry{}, false
	}
	return mountEntry{identifier: identifier, point: mountPoint(fields[4])}, true
}

func digits(text string) bool {
	return text != "" && strings.Trim(text, "0123456789") == ""
}

// An escape above 0o377 stays literal.
func mountPoint(field string) string {
	decoded := make([]byte, 0, len(field))
	for index := 0; index < len(field); {
		if field[index] == '\\' && index+4 <= len(field) {
			if value, ok := octalByte(field[index+1 : index+4]); ok {
				decoded = append(decoded, value)
				index += 4
				continue
			}
		}
		decoded = append(decoded, field[index])
		index++
	}
	return string(decoded)
}

func octalByte(text string) (byte, bool) {
	value := 0
	for index := range len(text) {
		if text[index] < '0' || text[index] > '7' {
			return 0, false
		}
		value = value*8 + int(text[index]-'0')
	}
	if value > 0xff {
		return 0, false
	}
	return byte(value), true
}

// The last listed wins among equal lengths, as a stacked mount does.
func mountID(entries []mountEntry, path string) (uint64, bool) {
	var selected uint64
	best := -1
	for _, entry := range entries {
		length := len(pathx.Components(entry.point))
		if pathx.HasPrefix(path, entry.point) && length >= best {
			selected, best = entry.identifier, length
		}
	}
	return selected, best >= 0
}
