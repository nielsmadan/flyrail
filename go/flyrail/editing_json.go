package flyrail

import (
	"cmp"
	"errors"
	"fmt"
	"slices"
	"strconv"
	"strings"
	"unicode/utf8"

	"github.com/nielsmadan/flyrail/go/internal/jsonx"
)

const (
	jsonMaxDepth    = 985
	jsonNonFinite   = "floats must be finite"
	jsonSurrogates  = "scalar must contain Unicode scalar values"
	jsonMalformed   = "unterminated or malformed JSON container"
	jsonKeysStrings = "JSON object keys must be strings"
	jsonMissing     = "document selector path is missing"
)

type jsonChild struct {
	part     pathPart
	node     *jsonNode
	keyStart int
}

type jsonNode struct {
	start, end int
	value      Value
	children   []jsonChild
	commas     []int
}

type jsonDocument struct {
	source   string
	comments bool
	root     *jsonNode
}

func parseJSONDocument(text string, comments bool) (*jsonDocument, error) {
	scanner := jsonScanner{text: text, comments: comments}
	root, err := scanner.node(0)
	if err != nil {
		return nil, err
	}
	if err := scanner.space(); err != nil {
		return nil, err
	}
	if scanner.pos != len(text) {
		return nil, newValueError("unexpected data after JSON document")
	}
	return &jsonDocument{source: text, comments: comments, root: root}, nil
}

func partsMatch(key, part pathPart) bool {
	return key.isIndex == part.isIndex && key.key == part.key && key.index == part.index
}

func (d *jsonDocument) node(path []pathPart) (*jsonNode, error) {
	node := d.root
	for _, part := range path {
		index := slices.IndexFunc(node.children, func(child jsonChild) bool { return partsMatch(child.part, part) })
		if index < 0 {
			return nil, newValueError(jsonMissing)
		}
		node = node.children[index].node
	}
	return node, nil
}

type jsonPatch struct {
	start, end  int
	replacement string
}

// Keeps the rewritten text even when it no longer parses, as the reference does.
func (d *jsonDocument) patch(patches []jsonPatch) error {
	ordered := slices.Clone(patches)
	slices.SortStableFunc(ordered, func(a, b jsonPatch) int {
		return cmp.Or(cmp.Compare(b.start, a.start), cmp.Compare(b.end, a.end))
	})
	text := d.source
	for _, item := range ordered {
		if item.start < 0 || item.start > item.end || item.end > len(text) {
			return newValueError(jsonMissing)
		}
		text = text[:item.start] + item.replacement + text[item.end:]
	}
	d.source = text
	parsed, err := parseJSONDocument(text, d.comments)
	if err != nil {
		return err
	}
	d.root = parsed.root
	return nil
}

func (n *jsonNode) entryStart(index int) int {
	if index == 0 {
		return n.start + 1
	}
	if index-1 < len(n.commas) {
		return n.commas[index-1] + 1
	}
	return n.start + 1
}

func (d *jsonDocument) insertAt(parentPath []pathPart, index int, raw string, prefix []byte) error {
	parent, err := d.node(parentPath)
	if err != nil {
		return err
	}
	if index < len(parent.children) {
		start := parent.entryStart(index)
		start, err := d.prefixed(start, parent.children[index].keyStart, present(prefix))
		if err != nil {
			return err
		}
		return d.patch([]jsonPatch{{start, start, raw + ","}})
	}
	start := parent.end - 1
	if prefix != nil {
		var boundary int
		switch {
		case len(parent.commas) > 0 && len(parent.commas) == len(parent.children):
			boundary = parent.commas[len(parent.commas)-1] + 1
		case len(parent.children) > 0:
			boundary = parent.children[len(parent.children)-1].node.end
		default:
			boundary = parent.start + 1
		}
		if start, err = d.prefixed(boundary, start, prefix); err != nil {
			return err
		}
	}
	switch {
	case len(parent.children) > 0 && len(parent.commas) < len(parent.children):
		end := parent.children[len(parent.children)-1].node.end
		return d.patch([]jsonPatch{{start, start, raw}, {end, end, ","}})
	case len(parent.commas) > 0:
		return d.patch([]jsonPatch{{start, start, raw + ","}})
	}
	return d.patch([]jsonPatch{{start, start, raw}})
}

func (d *jsonDocument) prefixed(start, end int, prefix []byte) (int, error) {
	if start < 0 || start > end || end > len(d.source) || !strings.HasPrefix(d.source[start:end], string(prefix)) {
		return 0, newValueError("original array position trivia changed")
	}
	return start + len(prefix), nil
}

func (d *jsonDocument) text() string { return d.source }

func (d *jsonDocument) clone() documentBackend {
	copied := *d
	return &copied
}

func (d *jsonDocument) native(path []pathPart) (Value, error) {
	node, err := d.node(path)
	if err != nil {
		return nil, err
	}
	return node.value, nil
}

func (d *jsonDocument) syntax(path []pathPart) ([]byte, error) {
	node, err := d.node(path)
	if err != nil {
		return nil, err
	}
	return []byte(d.source[node.start:node.end]), nil
}

func (d *jsonDocument) ownedSyntax(path []pathPart) ([]byte, error) { return d.syntax(path) }

func (d *jsonDocument) layout([]pathPart) ([]byte, error) { return []byte{}, nil }

func (d *jsonDocument) positionPrefix(path []pathPart) ([]byte, error) {
	parentPath, index, ok := splitIndex(path)
	if !ok {
		return nil, newValueError("array position requires a member")
	}
	parent, err := d.node(parentPath)
	if err != nil {
		return nil, err
	}
	start := parent.entryStart(index)
	node, err := d.node(path)
	if err != nil {
		return nil, err
	}
	if start > node.start {
		return []byte{}, nil
	}
	return []byte(d.source[start:node.start]), nil
}

func (d *jsonDocument) emptySyntax(path []pathPart) ([]byte, error) { return d.syntax(path) }

func (d *jsonDocument) raw(value Value, syntax []byte) (string, error) {
	if syntax != nil {
		return decodeUTF8(syntax)
	}
	out, err := appendJSONValue(nil, value, 0)
	return string(out), err
}

func (d *jsonDocument) put(path []pathPart, value Value, syntax, _ []byte) error {
	raw, err := d.raw(value, syntax)
	if err != nil {
		return err
	}
	if len(path) == 0 {
		return d.patch([]jsonPatch{{d.root.start, d.root.end, raw}})
	}
	parentPath, last := path[:len(path)-1], path[len(path)-1]
	parent, err := d.node(parentPath)
	if err != nil {
		return err
	}
	if object, ok := parent.value.(Object); ok && !last.isIndex {
		if _, exists := object.Get(last.key); !exists {
			member := append(jsonx.AppendEscaped(nil, last.key), ':')
			return d.insertAt(parentPath, len(parent.children), string(member)+raw, nil)
		}
	}
	node, err := d.node(path)
	if err != nil {
		return err
	}
	return d.patch([]jsonPatch{{node.start, node.end, raw}})
}

func (d *jsonDocument) insert(path []pathPart, index int, value Value, syntax, prefix, _ []byte) error {
	raw, err := d.raw(value, syntax)
	if err != nil {
		return err
	}
	return d.insertAt(path, index, raw, prefix)
}

func (d *jsonDocument) delete(path []pathPart) error {
	if len(path) == 0 {
		return newValueError(jsonMissing)
	}
	parentPath, last := path[:len(path)-1], path[len(path)-1]
	parent, err := d.node(parentPath)
	if err != nil {
		return err
	}
	index := slices.IndexFunc(parent.children, func(child jsonChild) bool { return partsMatch(child.part, last) })
	if index < 0 {
		return newValueError(jsonMissing)
	}
	child := parent.children[index]
	patches := []jsonPatch{{child.node.start, child.node.end, ""}}
	if !last.isIndex {
		scanner := jsonScanner{text: d.source, pos: child.keyStart, comments: d.comments}
		if _, err := scanner.scanString(); err != nil {
			return err
		}
		keyEnd := scanner.pos
		if err := scanner.space(); err != nil {
			return err
		}
		patches = append(patches, jsonPatch{child.keyStart, keyEnd, ""}, jsonPatch{scanner.pos, scanner.pos + 1, ""})
	}
	switch {
	case index < len(parent.commas):
		patches = append(patches, jsonPatch{parent.commas[index], parent.commas[index] + 1, ""})
	case len(parent.commas) > 0:
		comma := parent.commas[len(parent.commas)-1]
		patches = append(patches, jsonPatch{comma, comma + 1, ""})
	}
	return d.patch(patches)
}

func (d *jsonDocument) relocate(path []pathPart, index int, value Value, syntax, prefix, layout []byte) error {
	if err := d.delete(path); err != nil {
		return err
	}
	return d.insert(path[:len(path)-1], index, value, syntax, prefix, layout)
}

func appendJSONValue(out []byte, value Value, depth int) ([]byte, error) {
	if depth > jsonMaxDepth {
		return nil, newNestingError(documentNesting)
	}
	switch v := value.(type) {
	case Null:
		return append(out, "null"...), nil
	case Bool:
		return strconv.AppendBool(out, bool(v)), nil
	case Int:
		return strconv.AppendInt(out, int64(v), 10), nil
	case Float:
		return append(out, jsonx.PythonFloatRepr(v.Float64())...), nil
	case String:
		return jsonx.AppendEscaped(out, v.text), nil
	case Date:
		return nil, newTypeError("Object of type date is not JSON serializable")
	case Time:
		return nil, newTypeError("Object of type time is not JSON serializable")
	case DateTime:
		return nil, newTypeError("Object of type datetime is not JSON serializable")
	case Array:
		out = append(out, '[')
		for index, item := range v.items {
			if index > 0 {
				out = append(out, ',')
			}
			var err error
			if out, err = appendJSONValue(out, item, depth+1); err != nil {
				return nil, err
			}
		}
		return append(out, ']'), nil
	case Object:
		out = append(out, '{')
		for index, member := range v.members {
			if index > 0 {
				out = append(out, ',')
			}
			out = append(jsonx.AppendEscaped(out, member.Key), ':')
			var err error
			if out, err = appendJSONValue(out, member.Value, depth+1); err != nil {
				return nil, err
			}
		}
		return append(out, '}'), nil
	}
	return nil, newValueError("values must be Scalar, ObjectValue or ArrayValue")
}

type jsonScanner struct {
	text     string
	pos      int
	comments bool
}

type jsonFrame struct {
	opening    byte
	start      int
	depth      int
	entryStart int
	nextKey    *string
	children   []jsonChild
	commas     []int
	members    []ObjectMember
	items      []Value
}

func (f *jsonFrame) closing() byte {
	if f.opening == '{' {
		return '}'
	}
	return ']'
}

func (f *jsonFrame) attach(node *jsonNode) error {
	if f.opening == '{' {
		if f.nextKey == nil {
			return newValueError("missing JSON object colon")
		}
		name := *f.nextKey
		f.nextKey = nil
		f.members = append(f.members, ObjectMember{Key: name, Value: node.value})
		f.children = append(f.children, jsonChild{part: segmentKey(name), node: node, keyStart: f.entryStart})
		return nil
	}
	f.children = append(f.children, jsonChild{part: segmentIndex(len(f.items)), node: node, keyStart: f.entryStart})
	f.items = append(f.items, node.value)
	return nil
}

func (f *jsonFrame) finish(end int) (*jsonNode, error) {
	var value Value
	var err error
	if f.opening == '{' {
		value, err = NewObject(f.members...)
	} else {
		value, err = NewArray(f.items...)
	}
	if err != nil {
		return nil, err
	}
	return &jsonNode{start: f.start, end: end, value: value, children: f.children, commas: f.commas}, nil
}

func (s *jsonScanner) byteAt(pos int) (byte, bool) {
	if pos < 0 || pos >= len(s.text) {
		return 0, false
	}
	return s.text[pos], true
}

func (s *jsonScanner) isAt(pos int, want byte) bool {
	b, ok := s.byteAt(pos)
	return ok && b == want
}

func (s *jsonScanner) space() error {
	pos, err := s.skipSpace(s.pos)
	s.pos = pos
	return err
}

func (s *jsonScanner) skipSpace(from int) (int, error) {
	pos := from
	for pos < len(s.text) {
		switch b := s.text[pos]; {
		case b == ' ' || b == '\t' || b == '\r' || b == '\n':
			pos++
		case b == '/' && s.comments && s.isAt(pos+1, '/'):
			offset := strings.IndexAny(s.text[pos+2:], "\r\n")
			if offset < 0 {
				return len(s.text), nil
			}
			pos += 2 + offset
		case b == '/' && s.comments && s.isAt(pos+1, '*'):
			offset := strings.Index(s.text[pos+2:], "*/")
			if offset < 0 {
				return pos, newValueError("unterminated JSONC comment")
			}
			pos += 2 + offset + 2
		default:
			return pos, nil
		}
	}
	return pos, nil
}

func (s *jsonScanner) node(depth int) (*jsonNode, error) {
	if err := s.space(); err != nil {
		return nil, err
	}
	start := s.pos
	opening, ok := s.byteAt(start)
	if !ok {
		return nil, newValueError("missing JSON value")
	}
	if opening != '[' && opening != '{' {
		value, end, err := s.scalar()
		if err != nil {
			return nil, err
		}
		return &jsonNode{start: start, end: end, value: value}, nil
	}
	s.pos++
	var stack []*jsonFrame
	current := &jsonFrame{opening: opening, start: start, depth: depth}
	entry := true
	for {
		if err := s.space(); err != nil {
			return nil, err
		}
		if entry {
			b, ok := s.byteAt(s.pos)
			if !ok {
				return nil, newValueError(jsonMalformed)
			}
			if b == current.closing() {
				s.pos++
				node, err := current.finish(s.pos)
				if err != nil {
					return nil, err
				}
				if len(stack) == 0 {
					return node, nil
				}
				current, stack = stack[len(stack)-1], stack[:len(stack)-1]
				if err := current.attach(node); err != nil {
					return nil, err
				}
				entry = false
				continue
			}
			current.entryStart = s.pos
			if current.opening == '{' {
				name, err := s.scanKey(current.depth + 1)
				if err != nil {
					return nil, err
				}
				if slices.ContainsFunc(current.members, func(member ObjectMember) bool { return member.Key == name }) {
					return nil, newValueError("duplicate JSON object key")
				}
				if err := s.space(); err != nil {
					return nil, err
				}
				if !s.isAt(s.pos, ':') {
					return nil, newValueError("missing JSON object colon")
				}
				s.pos++
				current.nextKey = &name
			}
			if err := s.space(); err != nil {
				return nil, err
			}
			valueStart := s.pos
			valueByte, ok := s.byteAt(valueStart)
			if !ok {
				return nil, newValueError("missing JSON value")
			}
			if valueByte == '[' || valueByte == '{' {
				if current.depth+1 >= jsonMaxDepth {
					return nil, newNestingError(documentNesting)
				}
				s.pos++
				stack = append(stack, current)
				current = &jsonFrame{opening: valueByte, start: valueStart, depth: current.depth + 1}
				continue
			}
			value, end, err := s.scalar()
			if err != nil {
				return nil, err
			}
			if err := current.attach(&jsonNode{start: valueStart, end: end, value: value}); err != nil {
				return nil, err
			}
			entry = false
			continue
		}
		if !s.isAt(s.pos, ',') {
			if !s.isAt(s.pos, current.closing()) {
				return nil, newValueError(jsonMalformed)
			}
			s.pos++
			node, err := current.finish(s.pos)
			if err != nil {
				return nil, err
			}
			if len(stack) == 0 {
				return node, nil
			}
			current, stack = stack[len(stack)-1], stack[:len(stack)-1]
			if err := current.attach(node); err != nil {
				return nil, err
			}
			continue
		}
		current.commas = append(current.commas, s.pos)
		s.pos++
		if err := s.space(); err != nil {
			return nil, err
		}
		if !s.comments && s.isAt(s.pos, current.closing()) {
			return nil, newValueError("JSON trailing commas are unsupported")
		}
		entry = true
	}
}

func (s *jsonScanner) scanKey(depth int) (string, error) {
	b, ok := s.byteAt(s.pos)
	switch {
	case !ok:
		return "", newValueError("missing JSON value")
	case b == '"':
		return s.scanString()
	case b == '[' || b == '{':
		if depth >= jsonMaxDepth {
			return "", newNestingError(documentNesting)
		}
		if _, err := s.node(depth); err != nil {
			return "", err
		}
		return "", newValueError(jsonKeysStrings)
	}
	value, _, err := s.scalar()
	if err != nil {
		return "", err
	}
	text, ok := value.(String)
	if !ok {
		return "", newValueError(jsonKeysStrings)
	}
	return text.text, nil
}

func (s *jsonScanner) scalar() (Value, int, error) {
	start := s.pos
	b, _ := s.byteAt(start)
	switch {
	case b == '"':
		text, err := s.scanString()
		if err != nil {
			return nil, 0, err
		}
		return String{text: text}, s.pos, nil
	case b == 't':
		return s.literal("true", Bool(true), nil)
	case b == 'f':
		return s.literal("false", Bool(false), nil)
	case b == 'n':
		return s.literal("null", Null{}, nil)
	case b == 'N':
		return s.literal("NaN", nil, newValueError(jsonNonFinite))
	case b == 'I':
		return s.literal("Infinity", nil, newValueError(jsonNonFinite))
	case b == '-':
		if strings.HasPrefix(s.text[start+1:], "Infinity") {
			return nil, 0, newValueError(jsonNonFinite)
		}
		return s.number()
	case b >= '0' && b <= '9':
		return s.number()
	}
	return nil, 0, jsonDecodeError(s.text, "Expecting value", start)
}

func (s *jsonScanner) literal(word string, value Value, failure error) (Value, int, error) {
	start := s.pos
	if !strings.HasPrefix(s.text[start:], word) {
		return nil, 0, jsonDecodeError(s.text, "Expecting value", start)
	}
	s.pos += len(word)
	if failure != nil {
		return nil, 0, failure
	}
	return value, s.pos, nil
}

func (s *jsonScanner) digit(pos int) bool {
	b, ok := s.byteAt(pos)
	return ok && b >= '0' && b <= '9'
}

func (s *jsonScanner) number() (Value, int, error) {
	start := s.pos
	pos := start
	if s.isAt(pos, '-') {
		pos++
	}
	switch b, _ := s.byteAt(pos); {
	case b == '0':
		pos++
	case b >= '1' && b <= '9':
		for s.digit(pos) {
			pos++
		}
	default:
		return nil, 0, jsonDecodeError(s.text, "Expecting value", start)
	}
	floating := false
	if s.isAt(pos, '.') && s.digit(pos+1) {
		floating = true
		pos++
		for s.digit(pos) {
			pos++
		}
	}
	if b, _ := s.byteAt(pos); b == 'e' || b == 'E' {
		end := pos + 1
		if sign, _ := s.byteAt(end); sign == '+' || sign == '-' {
			end++
		}
		if s.digit(end) {
			pos = end
			for s.digit(pos) {
				pos++
			}
			floating = true
		}
	}
	text := s.text[start:pos]
	var value Value
	if floating {
		parsed, err := strconv.ParseFloat(text, 64)
		if err != nil && !errors.Is(err, strconv.ErrRange) {
			return nil, 0, newValueError(jsonNonFinite)
		}
		number, err := NewFloat(parsed)
		if err != nil {
			return nil, 0, err
		}
		value = number
	} else {
		parsed, err := strconv.ParseInt(text, 10, 64)
		if err != nil {
			return nil, 0, newValueError("integers must fit signed 64 bits")
		}
		value = Int(parsed)
	}
	s.pos = pos
	return value, s.pos, nil
}

func (s *jsonScanner) scanString() (string, error) {
	opening := s.pos
	pos := opening + 1
	var out strings.Builder
	surrogate := false
	unterminated := func() error { return jsonDecodeError(s.text, "Unterminated string starting at", opening) }
	for {
		b, ok := s.byteAt(pos)
		if !ok {
			return "", unterminated()
		}
		switch {
		case b == '"':
			s.pos = pos + 1
			if surrogate {
				return "", newValueError(jsonSurrogates)
			}
			return out.String(), nil
		case b == '\\':
			pos++
			escaped, ok := s.byteAt(pos)
			if !ok {
				return "", unterminated()
			}
			if simple, known := jsonSimpleEscapes[escaped]; known {
				out.WriteByte(simple)
				pos++
				continue
			}
			if escaped != 'u' {
				return "", jsonDecodeError(s.text, "Invalid \\escape", pos-1)
			}
			code, ok := s.hex4(pos + 1)
			if !ok {
				return "", jsonDecodeError(s.text, "Invalid \\uXXXX escape", pos)
			}
			pos += 5
			switch {
			case code >= 0xD800 && code <= 0xDBFF:
				second, paired := s.hex4(pos + 2)
				if strings.HasPrefix(s.text[pos:], "\\u") && paired && second >= 0xDC00 && second <= 0xDFFF {
					out.WriteRune(0x10000 + (code-0xD800)<<10 + (second - 0xDC00))
					pos += 6
				} else {
					surrogate = true
				}
			case code >= 0xDC00 && code <= 0xDFFF:
				surrogate = true
			default:
				out.WriteRune(code)
			}
		case b <= 0x1F:
			return "", jsonDecodeError(s.text, "Invalid control character at", pos)
		default:
			character, size := utf8.DecodeRuneInString(s.text[pos:])
			if character == utf8.RuneError && size <= 1 {
				return "", unterminated()
			}
			out.WriteRune(character)
			pos += size
		}
	}
}

var jsonSimpleEscapes = map[byte]byte{
	'"': '"', '\\': '\\', '/': '/', 'b': '\b', 'f': '\f', 'n': '\n', 'r': '\r', 't': '\t',
}

func (s *jsonScanner) hex4(pos int) (rune, bool) {
	if pos < 0 || pos+4 > len(s.text) {
		return 0, false
	}
	var value rune
	for _, digit := range []byte(s.text[pos : pos+4]) {
		var nibble byte
		switch {
		case digit >= '0' && digit <= '9':
			nibble = digit - '0'
		case digit >= 'a' && digit <= 'f':
			nibble = digit - 'a' + 10
		case digit >= 'A' && digit <= 'F':
			nibble = digit - 'A' + 10
		default:
			return 0, false
		}
		value = value*16 + rune(nibble)
	}
	return value, true
}

func jsonDecodeError(text, message string, bytePos int) *ValueError {
	if bytePos < 0 || bytePos > len(text) {
		return newValueError(message)
	}
	prefix := text[:bytePos]
	charPos := utf8.RuneCountInString(prefix)
	line := strings.Count(prefix, "\n") + 1
	column := charPos + 1
	if index := strings.LastIndexByte(prefix, '\n'); index >= 0 {
		column = utf8.RuneCountInString(text[index:bytePos])
	}
	return newValueError(fmt.Sprintf("%s: line %d column %d (char %d)", message, line, column, charPos))
}
