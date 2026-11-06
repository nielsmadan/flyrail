package jsonx

import (
	"math"
	"strconv"
	"strings"
	"unicode/utf8"
)

type style struct {
	listSeparator string
	pairSeparator string
	ensureASCII   bool
}

var (
	compact       = style{listSeparator: ",", pairSeparator: ":"}
	python        = style{listSeparator: ", ", pairSeparator: ": ", ensureASCII: true}
	pythonCompact = style{listSeparator: ",", pairSeparator: ":", ensureASCII: true}
)

// Encode writes v as compact JSON with literal Unicode, as Python's json.dumps(ensure_ascii=False, separators=(",", ":")).
func Encode(v Value) []byte {
	return AppendEncode(nil, v)
}

// AppendEncode appends the Encode form of v to dst.
func AppendEncode(dst []byte, v Value) []byte {
	return compact.append(dst, v)
}

// EncodePython writes v as Python's default json.dumps: ASCII-only with ", " and ": " separators.
func EncodePython(v Value) []byte {
	return python.append(nil, v)
}

// EncodePythonCompact writes v as Python's json.dumps(separators=(",", ":")) with ASCII-only escapes.
func EncodePythonCompact(v Value) []byte {
	return pythonCompact.append(nil, v)
}

func (s style) append(dst []byte, v Value) []byte {
	switch v.kind {
	case KindNull:
		return append(dst, "null"...)
	case KindBool:
		if v.boolean {
			return append(dst, "true"...)
		}
		return append(dst, "false"...)
	case KindInt:
		return strconv.AppendInt(dst, v.integer, 10)
	case KindBigInt:
		return append(dst, v.text...)
	case KindFloat:
		return append(dst, PythonFloatRepr(v.float)...)
	case KindString:
		return appendEscaped(dst, v.text, s.ensureASCII)
	case KindArray:
		dst = append(dst, '[')
		for index, item := range v.items {
			if index > 0 {
				dst = append(dst, s.listSeparator...)
			}
			dst = s.append(dst, item)
		}
		return append(dst, ']')
	case KindObject:
		dst = append(dst, '{')
		for index, member := range v.members {
			if index > 0 {
				dst = append(dst, s.listSeparator...)
			}
			dst = appendEscaped(dst, member.Key, s.ensureASCII)
			dst = append(dst, s.pairSeparator...)
			dst = s.append(dst, member.Value)
		}
		return append(dst, '}')
	}
	return dst
}

// AppendEscaped appends s as a quoted JSON string with literal Unicode and lowercase \u escapes for control characters.
func AppendEscaped(dst []byte, s string) []byte {
	return appendEscaped(dst, s, false)
}

const hexDigits = "0123456789abcdef"

func appendUnit(dst []byte, unit rune) []byte {
	return append(dst, '\\', 'u',
		hexDigits[unit>>12&0xF], hexDigits[unit>>8&0xF], hexDigits[unit>>4&0xF], hexDigits[unit&0xF])
}

func appendEscaped(dst []byte, s string, ensureASCII bool) []byte {
	dst = append(dst, '"')
	for _, character := range s {
		switch {
		case character == '"':
			dst = append(dst, '\\', '"')
		case character == '\\':
			dst = append(dst, '\\', '\\')
		case character == '\b':
			dst = append(dst, '\\', 'b')
		case character == '\f':
			dst = append(dst, '\\', 'f')
		case character == '\n':
			dst = append(dst, '\\', 'n')
		case character == '\r':
			dst = append(dst, '\\', 'r')
		case character == '\t':
			dst = append(dst, '\\', 't')
		case character < 0x20:
			dst = appendUnit(dst, character)
		case ensureASCII && character >= utf8.RuneSelf:
			if character <= 0xFFFF {
				dst = appendUnit(dst, character)
			} else {
				adjusted := character - 0x10000
				dst = appendUnit(dst, 0xD800+adjusted>>10)
				dst = appendUnit(dst, 0xDC00+adjusted&0x3FF)
			}
		default:
			dst = utf8.AppendRune(dst, character)
		}
	}
	return append(dst, '"')
}

// PythonFloatRepr returns Python's repr of a binary64 value, with JSON's NaN and Infinity spellings.
func PythonFloatRepr(value float64) string {
	switch {
	case math.IsNaN(value):
		return "NaN"
	case math.IsInf(value, 1):
		return "Infinity"
	case math.IsInf(value, -1):
		return "-Infinity"
	case value == 0:
		if math.Signbit(value) {
			return "-0.0"
		}
		return "0.0"
	}
	shortest := strconv.FormatFloat(value, 'e', -1, 64)
	mantissa, exponentText, _ := strings.Cut(shortest, "e")
	exponent, _ := strconv.Atoi(exponentText)
	sign := ""
	if strings.HasPrefix(mantissa, "-") {
		sign = "-"
	}
	digits := strings.TrimRight(strings.Map(func(r rune) rune {
		if r >= '0' && r <= '9' {
			return r
		}
		return -1
	}, mantissa), "0")
	if digits == "" {
		digits = "0"
	}
	point := exponent + 1
	length := len(digits)
	switch {
	case point <= -4 || point > 16:
		tail := ""
		if len(digits) > 1 {
			tail = "." + digits[1:]
		}
		scale := point - 1
		scaleSign := "+"
		if scale < 0 {
			scaleSign = "-"
			scale = -scale
		}
		scaleText := strconv.Itoa(scale)
		if len(scaleText) < 2 {
			scaleText = "0" + scaleText
		}
		return sign + digits[:1] + tail + "e" + scaleSign + scaleText
	case point <= 0:
		return sign + "0." + strings.Repeat("0", -point) + digits
	case point >= length:
		return sign + digits + strings.Repeat("0", point-length) + ".0"
	}
	return sign + digits[:point] + "." + digits[point:]
}

// PythonStrRepr returns Python's repr of a string, escaping as Flyrail's reference implementation does.
func PythonStrRepr(text string) string {
	quote := '\''
	if strings.ContainsRune(text, '\'') && !strings.ContainsRune(text, '"') {
		quote = '"'
	}
	var out strings.Builder
	out.WriteRune(quote)
	for _, character := range text {
		switch {
		case character == '\\':
			out.WriteString(`\\`)
		case character == '\n':
			out.WriteString(`\n`)
		case character == '\r':
			out.WriteString(`\r`)
		case character == '\t':
			out.WriteString(`\t`)
		case character < 0x20 || character == 0x7F:
			out.WriteString(`\x`)
			out.WriteByte(hexDigits[character>>4])
			out.WriteByte(hexDigits[character&0xF])
		case character == quote:
			out.WriteByte('\\')
			out.WriteRune(character)
		default:
			out.WriteRune(character)
		}
	}
	out.WriteRune(quote)
	return out.String()
}

// PythonRepr returns Python's repr of the value a JSON document decodes to.
func PythonRepr(v Value) string {
	switch v.kind {
	case KindNull:
		return "None"
	case KindBool:
		if v.boolean {
			return "True"
		}
		return "False"
	case KindInt:
		return strconv.FormatInt(v.integer, 10)
	case KindBigInt:
		return v.text
	case KindFloat:
		return PythonFloatRepr(v.float)
	case KindString:
		return PythonStrRepr(v.text)
	case KindArray:
		parts := make([]string, 0, len(v.items))
		for _, item := range v.items {
			parts = append(parts, PythonRepr(item))
		}
		return "[" + strings.Join(parts, ", ") + "]"
	case KindObject:
		parts := make([]string, 0, len(v.members))
		for _, member := range v.members {
			parts = append(parts, PythonStrRepr(member.Key)+": "+PythonRepr(member.Value))
		}
		return "{" + strings.Join(parts, ", ") + "}"
	}
	return ""
}
