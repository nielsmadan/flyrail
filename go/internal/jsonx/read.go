package jsonx

import (
	"bytes"
	"errors"
	"strconv"
	"unicode/utf8"
)

// MaxDepth is the deepest container nesting Parse accepts below the top-level value.
const MaxDepth = 512

// Parse reads one strict JSON document: duplicate names, unpaired surrogates, invalid UTF-8,
// non-JSON constants, trailing content and nesting deeper than MaxDepth are errors.
func Parse(data []byte) (Value, error) {
	reader := parser{data: data}
	value, err := reader.value(0)
	if err != nil {
		return Value{}, err
	}
	reader.skipWhitespace()
	if reader.pos != len(reader.data) {
		return Value{}, fail("JSON has trailing content")
	}
	return value, nil
}

type parser struct {
	data []byte
	pos  int
}

func (p *parser) peek() (byte, bool) {
	if p.pos < len(p.data) {
		return p.data[p.pos], true
	}
	return 0, false
}

func (p *parser) peekIs(b byte) bool {
	next, ok := p.peek()
	return ok && next == b
}

func (p *parser) peekDigit() bool {
	next, ok := p.peek()
	return ok && next >= '0' && next <= '9'
}

func (p *parser) hasPrefix(prefix string) bool {
	return bytes.HasPrefix(p.data[p.pos:], []byte(prefix))
}

func (p *parser) skipWhitespace() {
	for p.pos < len(p.data) {
		switch p.data[p.pos] {
		case ' ', '\t', '\n', '\r':
			p.pos++
		default:
			return
		}
	}
}

func (p *parser) value(depth int) (Value, *Error) {
	if depth > MaxDepth {
		return Value{}, fail("JSON nesting is too deep")
	}
	next, _ := p.peek()
	switch {
	case next == '{':
		return p.object(depth)
	case next == '[':
		return p.array(depth)
	case next == '"':
		text, err := p.string()
		return String(text), err
	case next == 't':
		return p.literal("true", Bool(true))
	case next == 'f':
		return p.literal("false", Bool(false))
	case next == 'n':
		return p.literal("null", Null())
	case next == 'N' && p.hasPrefix("NaN"):
		return Value{}, fail("non-JSON numeric constant: NaN")
	case next == 'I' && p.hasPrefix("Infinity"):
		return Value{}, fail("non-JSON numeric constant: Infinity")
	case next == '-' || (next >= '0' && next <= '9'):
		return p.number()
	}
	return Value{}, fail("JSON contains an unexpected character")
}

func (p *parser) literal(word string, value Value) (Value, *Error) {
	if p.hasPrefix(word) {
		p.pos += len(word)
		return value, nil
	}
	return Value{}, fail("JSON contains an invalid literal")
}

func (p *parser) array(depth int) (Value, *Error) {
	p.pos++
	items := []Value{}
	p.skipWhitespace()
	if p.peekIs(']') {
		p.pos++
		return Array(items...), nil
	}
	for {
		item, err := p.value(depth + 1)
		if err != nil {
			return Value{}, err
		}
		items = append(items, item)
		p.skipWhitespace()
		next, _ := p.peek()
		switch next {
		case ',':
			p.pos++
			p.skipWhitespace()
		case ']':
			p.pos++
			return Array(items...), nil
		default:
			return Value{}, fail("JSON array is missing a separator")
		}
	}
}

func (p *parser) object(depth int) (Value, *Error) {
	p.pos++
	members := []Member{}
	seen := map[string]struct{}{}
	p.skipWhitespace()
	if p.peekIs('}') {
		p.pos++
		return Object(members...), nil
	}
	for {
		if !p.peekIs('"') {
			return Value{}, fail("JSON object keys must be strings")
		}
		key, err := p.string()
		if err != nil {
			return Value{}, err
		}
		p.skipWhitespace()
		if !p.peekIs(':') {
			return Value{}, fail("JSON object is missing a colon")
		}
		p.pos++
		p.skipWhitespace()
		item, err := p.value(depth + 1)
		if err != nil {
			return Value{}, err
		}
		if _, duplicate := seen[key]; duplicate {
			return Value{}, fail("duplicate JSON field: " + key)
		}
		seen[key] = struct{}{}
		members = append(members, Member{Key: key, Value: item})
		p.skipWhitespace()
		next, _ := p.peek()
		switch next {
		case ',':
			p.pos++
			p.skipWhitespace()
		case '}':
			p.pos++
			return Object(members...), nil
		default:
			return Value{}, fail("JSON object is missing a separator")
		}
	}
}

func (p *parser) string() (string, *Error) {
	p.pos++
	var out []byte
	for {
		next, ok := p.peek()
		if !ok {
			return "", fail("JSON string is unterminated")
		}
		switch {
		case next == '"':
			p.pos++
			return string(out), nil
		case next == '\\':
			p.pos++
			var err *Error
			if out, err = p.escape(out); err != nil {
				return "", err
			}
		case next < 0x20:
			return "", fail("JSON string contains a control character")
		default:
			length := utf8Length(next)
			end := p.pos + length
			if length == 0 || end > len(p.data) || !utf8.Valid(p.data[p.pos:end]) {
				return "", fail("JSON string contains invalid UTF-8")
			}
			out = append(out, p.data[p.pos:end]...)
			p.pos = end
		}
	}
}

func (p *parser) escape(out []byte) ([]byte, *Error) {
	next, ok := p.peek()
	if !ok {
		return nil, fail("JSON escape is unterminated")
	}
	p.pos++
	switch next {
	case '"', '\\', '/':
		return append(out, next), nil
	case 'b':
		return append(out, '\b'), nil
	case 'f':
		return append(out, '\f'), nil
	case 'n':
		return append(out, '\n'), nil
	case 'r':
		return append(out, '\r'), nil
	case 't':
		return append(out, '\t'), nil
	case 'u':
		first, err := p.hex4()
		if err != nil {
			return nil, err
		}
		switch {
		case first >= 0xD800 && first <= 0xDBFF:
			if !p.hasPrefix(`\u`) {
				return nil, fail("JSON escape contains an unpaired surrogate")
			}
			p.pos += 2
			second, err := p.hex4()
			if err != nil {
				return nil, err
			}
			if second < 0xDC00 || second > 0xDFFF {
				return nil, fail("JSON escape contains an unpaired surrogate")
			}
			return utf8.AppendRune(out, 0x10000+(first-0xD800)<<10+(second-0xDC00)), nil
		case first >= 0xDC00 && first <= 0xDFFF:
			return nil, fail("JSON escape contains an unpaired surrogate")
		}
		return utf8.AppendRune(out, first), nil
	}
	return nil, fail("JSON contains an invalid escape")
}

func (p *parser) hex4() (rune, *Error) {
	if p.pos+4 > len(p.data) {
		return 0, fail("JSON unicode escape is truncated")
	}
	var value rune
	for _, digit := range p.data[p.pos : p.pos+4] {
		var nibble byte
		switch {
		case digit >= '0' && digit <= '9':
			nibble = digit - '0'
		case digit >= 'a' && digit <= 'f':
			nibble = digit - 'a' + 10
		case digit >= 'A' && digit <= 'F':
			nibble = digit - 'A' + 10
		default:
			return 0, fail("JSON unicode escape is not hexadecimal")
		}
		value = value*16 + rune(nibble)
	}
	p.pos += 4
	return value, nil
}

func (p *parser) skipDigits() {
	for p.peekDigit() {
		p.pos++
	}
}

func (p *parser) number() (Value, *Error) {
	start := p.pos
	if p.peekIs('-') {
		p.pos++
		if p.hasPrefix("Infinity") {
			return Value{}, fail("non-JSON numeric constant: -Infinity")
		}
	}
	next, _ := p.peek()
	switch {
	case next == '0':
		p.pos++
		if p.peekDigit() {
			return Value{}, fail("JSON number has a leading zero")
		}
	case next >= '1' && next <= '9':
		p.skipDigits()
	default:
		return Value{}, fail("JSON number is malformed")
	}
	floating := false
	if p.peekIs('.') {
		floating = true
		p.pos++
		if !p.peekDigit() {
			return Value{}, fail("JSON number has an empty fraction")
		}
		p.skipDigits()
	}
	if p.peekIs('e') || p.peekIs('E') {
		floating = true
		p.pos++
		if p.peekIs('+') || p.peekIs('-') {
			p.pos++
		}
		if !p.peekDigit() {
			return Value{}, fail("JSON number has an empty exponent")
		}
		p.skipDigits()
	}
	text := string(p.data[start:p.pos])
	if floating {
		value, err := strconv.ParseFloat(text, 64)
		if err != nil && !errors.Is(err, strconv.ErrRange) {
			return Value{}, fail("JSON number is out of range")
		}
		return Float(value), nil
	}
	if value, err := strconv.ParseInt(text, 10, 64); err == nil {
		return Int(value), nil
	}
	return BigInt(text), nil
}

func utf8Length(lead byte) int {
	switch {
	case lead <= 0x7F:
		return 1
	case lead >= 0xC0 && lead <= 0xDF:
		return 2
	case lead >= 0xE0 && lead <= 0xEF:
		return 3
	case lead >= 0xF0 && lead <= 0xF7:
		return 4
	}
	return 0
}
