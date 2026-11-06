package flyrail

import (
	"errors"
	"fmt"
	"math"
	"strconv"
	"strings"
	"unicode/utf8"
)

const tomlCanonicalNaN = 0x7FF8_0000_0000_0000

func rawFloat(value float64) Float {
	if math.IsNaN(value) {
		return Float{bits: tomlCanonicalNaN}
	}
	return Float{bits: math.Float64bits(value)}
}

func parseTOMLEscape(s *tomlScanner, value *strings.Builder) error {
	code := s.cur()
	var decoded byte
	switch code {
	case tomlEOF:
		return s.parseError("Unexpected end of file")
	case 'b':
		decoded = '\b'
	case 't':
		decoded = '\t'
	case 'n':
		decoded = '\n'
	case 'f':
		decoded = '\f'
	case 'r':
		decoded = '\r'
	case 'e':
		decoded = 0x1B
	case '"':
		decoded = '"'
	case '\\':
		decoded = '\\'
	case 'u', 'U', 'x':
		digits := 2
		switch code {
		case 'u':
			digits = 4
		case 'U':
			digits = 8
		}
		text := s.src[s.pos+1 : min(s.pos+1+digits, len(s.src))]
		point, err := strconv.ParseUint(text, 16, 32)
		if len(text) != digits || !allHexDigits(text) || err != nil || point > utf8.MaxRune || point >= 0xD800 && point <= 0xDFFF {
			return s.parseError("Invalid unicode value")
		}
		for range digits + 1 {
			s.step()
		}
		value.WriteRune(rune(point))
		return nil
	default:
		character, _ := utf8.DecodeRuneInString(s.src[s.pos:])
		return s.parseError(fmt.Sprintf("Invalid character '%c' in string", character))
	}
	s.step()
	value.WriteByte(decoded)
	return nil
}

func isTOMLBareValueStop(code int) bool {
	switch code {
	case tomlEOF, ' ', '\t', '\n', '\r', ',', ']', '}', '#':
		return true
	}
	return false
}

func scanTOMLBareValue(s *tomlScanner) string {
	start := s.pos
	for _, literal := range []string{"true", "false"} {
		if strings.HasPrefix(s.src[start:], literal) {
			for range literal {
				s.step()
			}
			return literal
		}
	}
	for !isTOMLBareValueStop(s.cur()) {
		s.step()
	}
	return s.src[start:s.pos]
}

func isTOMLDateShape(raw string) bool {
	return len(raw) == 10 && raw[4] == '-' && raw[7] == '-' && allASCIIDigits(raw[:4]) && allASCIIDigits(raw[5:7]) && allASCIIDigits(raw[8:])
}

func isASCIIDigit(code int) bool {
	return code >= '0' && code <= '9'
}

func parseTOMLBareValue(raw string, s *tomlScanner) (Value, error) {
	switch raw {
	case "true":
		return Bool(true), nil
	case "false":
		return Bool(false), nil
	}
	if value, ok := parseTOMLNumber(raw); ok {
		return value, nil
	}
	if value, ok := parseTOMLTemporal(raw); ok {
		return value, nil
	}
	diagnostic := "Invalid number"
	switch {
	case len(raw) >= 10 && raw[4] == '-':
		if len(raw) == 10 {
			diagnostic = "Invalid date"
		} else if tomlDateTimeGrammar(raw) {
			diagnostic = "Invalid datetime"
		}
	case strings.Contains(raw, ":"):
		diagnostic = "Invalid time"
	}
	return nil, s.parseError(diagnostic)
}

func hasAnyPrefix(text string, prefixes ...string) bool {
	for _, prefix := range prefixes {
		if strings.HasPrefix(text, prefix) {
			return true
		}
	}
	return false
}

func parseTOMLNumber(raw string) (Value, bool) {
	sign, body := "", raw
	if strings.HasPrefix(raw, "+") || strings.HasPrefix(raw, "-") {
		sign, body = raw[:1], raw[1:]
	}
	switch body {
	case "inf":
		if sign == "-" {
			return rawFloat(math.Inf(-1)), true
		}
		return rawFloat(math.Inf(1)), true
	case "nan":
		return rawFloat(math.NaN()), true
	}
	if body == "" || !isASCIIDigit(int(body[0])) {
		return nil, false
	}
	if len(body) > 1 && body[0] == '0' && !hasAnyPrefix(body, "0.", "0o", "0x", "0b", "0e") {
		return nil, false
	}
	base, isDigit := 10, isASCIIDigit
	switch {
	case strings.HasPrefix(body, "0b"):
		base, isDigit = 2, func(code int) bool { return code == '0' || code == '1' }
	case strings.HasPrefix(body, "0o"):
		base, isDigit = 8, func(code int) bool { return code >= '0' && code <= '7' }
	case strings.HasPrefix(body, "0x"):
		base, isDigit = 16, func(code int) bool {
			return isASCIIDigit(code) || code >= 'a' && code <= 'f' || code >= 'A' && code <= 'F'
		}
	}
	if base != 10 && sign != "" {
		return nil, false
	}
	var cleaned strings.Builder
	for index := range len(body) {
		if body[index] == '_' && index > 0 && index+1 < len(body) && isDigit(int(body[index-1])) && isDigit(int(body[index+1])) {
			continue
		}
		cleaned.WriteByte(body[index])
	}
	clean := strings.ToLower(cleaned.String())
	if strings.Contains(clean, "_") {
		return nil, false
	}
	if mantissa, _, _ := strings.Cut(clean, "e"); strings.HasSuffix(clean, ".") || base != 16 && strings.HasSuffix(mantissa, ".") {
		return nil, false
	}
	if base != 10 {
		digits := clean[2:]
		if digits == "" || strings.ContainsAny(digits, "+-") {
			return nil, false
		}
		value, err := strconv.ParseInt(digits, base, 64)
		if err != nil {
			return nil, false
		}
		return Int(value), true
	}
	if !strings.ContainsAny(clean, ".e") {
		if strings.ContainsAny(clean, "+-") {
			return nil, false
		}
		value, err := strconv.ParseInt(sign+clean, 10, 64)
		if err != nil {
			return nil, false
		}
		return Int(value), true
	}
	if !pythonFloatGrammar(clean) {
		return nil, false
	}
	value, err := strconv.ParseFloat(sign+clean, 64)
	if err != nil && !errors.Is(err, strconv.ErrRange) {
		return nil, false
	}
	return rawFloat(value), true
}

func pythonFloatGrammar(text string) bool {
	mantissa, exponent, hasExponent := strings.Cut(text, "e")
	whole, fraction, _ := strings.Cut(mantissa, ".")
	if whole+fraction == "" || !allASCIIDigits(whole) || !allASCIIDigits(fraction) {
		return false
	}
	if !hasExponent {
		return true
	}
	if exponent != "" && (exponent[0] == '+' || exponent[0] == '-') {
		exponent = exponent[1:]
	}
	return exponent != "" && allASCIIDigits(exponent)
}

func allHexDigits(text string) bool {
	for index := range len(text) {
		code := text[index]
		if !isASCIIDigit(int(code)) && (code < 'a' || code > 'f') && (code < 'A' || code > 'F') {
			return false
		}
	}
	return true
}

func allASCIIDigits(text string) bool {
	for index := range len(text) {
		if !isASCIIDigit(int(text[index])) {
			return false
		}
	}
	return true
}

type tomlClock struct {
	hour, minute, second, microsecond int
}

func parseTOMLClock(text string) (tomlClock, bool) {
	if len(text) < 5 || text[2] != ':' || !allASCIIDigits(text[:2]) || !allASCIIDigits(text[3:5]) {
		return tomlClock{}, false
	}
	clock := tomlClock{hour: atoiDigits(text[:2]), minute: atoiDigits(text[3:5])}
	rest := text[5:]
	if rest != "" {
		if len(rest) < 3 || rest[0] != ':' || !allASCIIDigits(rest[1:3]) {
			return tomlClock{}, false
		}
		clock.second = atoiDigits(rest[1:3])
		rest = rest[3:]
		if rest != "" {
			fraction, ok := strings.CutPrefix(rest, ".")
			if !ok || fraction == "" || !allASCIIDigits(fraction) {
				return tomlClock{}, false
			}
			clock.microsecond = atoiDigits((fraction + "00000")[:6])
		}
	}
	if clock.hour > 23 || clock.minute > 59 || clock.second > 59 {
		return tomlClock{}, false
	}
	return clock, true
}

func atoiDigits(text string) int {
	value := 0
	for index := range len(text) {
		value = value*10 + int(text[index]-'0')
	}
	return value
}

func splitTOMLOffset(text string) (string, string) {
	if strings.HasSuffix(text, "Z") || strings.HasSuffix(text, "z") {
		return text[:len(text)-1], text[len(text)-1:]
	}
	if len(text) >= 6 && (text[len(text)-6] == '+' || text[len(text)-6] == '-') {
		return text[:len(text)-6], text[len(text)-6:]
	}
	return text, ""
}

func parseTOMLOffset(offset string) (int, bool) {
	if len(offset) != 6 || offset[3] != ':' || !allASCIIDigits(offset[1:3]) || !allASCIIDigits(offset[4:]) {
		return 0, false
	}
	hours, minutes := atoiDigits(offset[1:3]), atoiDigits(offset[4:])
	if hours > 23 || minutes > 59 {
		return 0, false
	}
	seconds := hours*3600 + minutes*60
	if offset[0] == '-' {
		seconds = -seconds
	}
	return seconds, true
}

func parseTOMLDate(text string) (Date, bool) {
	if len(text) != 10 || text[4] != '-' || text[7] != '-' || !allASCIIDigits(text[:4]) {
		return Date{}, false
	}
	date, err := ParseDate(text)
	return date, err == nil
}

func parseTOMLTemporal(raw string) (Value, bool) {
	if len(raw) >= 10 && raw[4] == '-' {
		date, ok := parseTOMLDate(raw[:10])
		if !ok {
			return nil, false
		}
		if len(raw) == 10 {
			return date, true
		}
		if raw[10] != 'T' && raw[10] != 't' && raw[10] != ' ' {
			return nil, false
		}
		timeText, offset := splitTOMLOffset(raw[11:])
		clock, ok := parseTOMLClock(timeText)
		if !ok {
			return nil, false
		}
		timeOfDay, err := NewTime(clock.hour, clock.minute, clock.second, clock.microsecond)
		if err != nil {
			return nil, false
		}
		switch offset {
		case "":
			return NewLocalDateTime(date, timeOfDay), true
		case "Z", "z":
			return NewUTCDateTime(date, timeOfDay), true
		}
		seconds, ok := parseTOMLOffset(offset)
		if !ok {
			return nil, false
		}
		datetime, err := NewDateTimeWithOffset(date, timeOfDay, seconds)
		return datetime, err == nil
	}
	if !strings.Contains(raw, ":") {
		return nil, false
	}
	clock, ok := parseTOMLClock(raw)
	if !ok {
		return nil, false
	}
	timeOfDay, err := NewTime(clock.hour, clock.minute, clock.second, clock.microsecond)
	return timeOfDay, err == nil
}

func tomlDateTimeGrammar(raw string) bool {
	if len(raw) < 11 || raw[10] != 'T' && raw[10] != 't' && raw[10] != ' ' {
		return false
	}
	timeText, offset := splitTOMLOffset(raw[11:])
	shapedOffset := len(offset) == 6 && offset[3] == ':' && allASCIIDigits(offset[1:3]) && allASCIIDigits(offset[4:])
	if offset != "" && offset != "Z" && offset != "z" && !shapedOffset {
		return false
	}
	if len(timeText) < 5 || timeText[2] != ':' || !allASCIIDigits(timeText[:2]) || !allASCIIDigits(timeText[3:5]) {
		return false
	}
	rest := timeText[5:]
	if rest == "" {
		return true
	}
	if len(rest) < 3 || rest[0] != ':' || !allASCIIDigits(rest[1:3]) {
		return false
	}
	digits, hasFraction := strings.CutPrefix(rest[3:], ".")
	return rest[3:] == "" || hasFraction && digits != "" && allASCIIDigits(digits)
}
