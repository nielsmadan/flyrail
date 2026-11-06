package flyrail

import (
	"errors"
	"fmt"
	"slices"
	"strings"
	"unicode/utf8"

	"github.com/nielsmadan/flyrail/go/internal/jsonx"
	"github.com/nielsmadan/flyrail/go/internal/unicodex"
)

const (
	tomlEmptyKey      = "Empty key"
	tomlMaxKeyDepth   = 100
	tomlMaxValueDepth = 100
	tomlEOF           = -1
)

type tomlKeyPart struct {
	name   string
	raw    string
	quoted bool
}

type tomlKey struct {
	parts  []tomlKeyPart
	dotted bool
	sep    string
}

func (k *tomlKey) name() string {
	if len(k.parts) == 0 {
		return ""
	}
	return k.parts[0].name
}

func (k *tomlKey) asString() string {
	raws := make([]string, len(k.parts))
	for index, part := range k.parts {
		raws[index] = part.raw
	}
	return strings.Join(raws, ".")
}

func (k *tomlKey) names() []string {
	names := make([]string, len(k.parts))
	for index, part := range k.parts {
		names[index] = part.name
	}
	return names
}

func (k *tomlKey) dottedString() string {
	return strings.Join(k.names(), ".")
}

func (k *tomlKey) clone() *tomlKey {
	if k == nil {
		return nil
	}
	return &tomlKey{parts: slices.Clone(k.parts), dotted: k.dotted, sep: k.sep}
}

func partKey(part tomlKeyPart, dotted bool) *tomlKey {
	return &tomlKey{parts: []tomlKeyPart{part}, dotted: dotted, sep: " = "}
}

type tomlTrivia struct {
	indent    string
	commentWS string
	comment   string
	trail     string
}

func defaultTrivia() tomlTrivia {
	return tomlTrivia{trail: "\n"}
}

type tomlContainer struct {
	body       []tomlEntry
	tableKeys  []string
	index      *tomlIndex
	validation map[string]*tomlValidation
	proxies    map[string]*tomlProxyCache
	mutations  int
}

type tomlEntry struct {
	key  *tomlKey
	item tomlItem
}

type tomlTable struct {
	body        tomlContainer
	trivia      tomlTrivia
	aotElement  bool
	implicit    bool
	displayName string
}

type tomlItemKind uint8

const (
	tomlNullItem tomlItemKind = iota
	tomlValueItem
	tomlArrayItem
	tomlInlineItem
	tomlTableItem
	tomlAOTItem
	tomlWhitespaceItem
	tomlCommentItem
)

type tomlInlineEntry struct {
	key  tomlKey
	item tomlItem
}

type tomlItem struct {
	kind    tomlItemKind
	raw     string
	value   Value
	trivia  tomlTrivia
	items   []tomlItem
	spans   [][2]int
	commas  []int
	entries []tomlInlineEntry
	table   tomlTable
	aot     []tomlTable
}

func whitespaceItem(raw string) tomlItem {
	return tomlItem{kind: tomlWhitespaceItem, raw: raw}
}

func commentItem(trivia tomlTrivia) tomlItem {
	return tomlItem{kind: tomlCommentItem, trivia: trivia}
}

func tableItem(table tomlTable) tomlItem {
	return tomlItem{kind: tomlTableItem, table: table}
}

func aotItem(members []tomlTable) tomlItem {
	return tomlItem{kind: tomlAOTItem, aot: members}
}

func (i *tomlItem) isTable() bool {
	return i.kind == tomlTableItem || i.kind == tomlAOTItem
}

func (i *tomlItem) isValueLike() bool {
	return i.kind == tomlValueItem || i.kind == tomlArrayItem || i.kind == tomlInlineItem
}

func (i *tomlItem) valueTrivia() *tomlTrivia {
	if i.isValueLike() {
		return &i.trivia
	}
	return nil
}

func (i *tomlItem) anyTrivia() *tomlTrivia {
	switch i.kind {
	case tomlValueItem, tomlArrayItem, tomlInlineItem, tomlCommentItem:
		return &i.trivia
	case tomlTableItem:
		return &i.table.trivia
	}
	return nil
}

func (i *tomlItem) clone() tomlItem {
	copied := *i
	copied.spans = slices.Clone(i.spans)
	copied.commas = slices.Clone(i.commas)
	copied.table = i.table.clone()
	if i.items != nil {
		copied.items = make([]tomlItem, len(i.items))
		for index := range i.items {
			copied.items[index] = i.items[index].clone()
		}
	}
	if i.entries != nil {
		copied.entries = make([]tomlInlineEntry, len(i.entries))
		for index, entry := range i.entries {
			copied.entries[index] = tomlInlineEntry{key: *entry.key.clone(), item: entry.item.clone()}
		}
	}
	copied.aot = cloneTables(i.aot)
	return copied
}

func (t *tomlTable) clone() tomlTable {
	copied := *t
	copied.body = t.body.clone()
	return copied
}

func cloneTables(tables []tomlTable) []tomlTable {
	if tables == nil {
		return nil
	}
	copied := make([]tomlTable, len(tables))
	for index := range tables {
		copied[index] = tables[index].clone()
	}
	return copied
}

func (c *tomlContainer) clone() tomlContainer {
	copied := tomlContainer{}
	if c.body != nil {
		copied.body = make([]tomlEntry, len(c.body))
		for index := range c.body {
			copied.body[index] = tomlEntry{key: c.body[index].key.clone(), item: c.body[index].item.clone()}
		}
	}
	copied.tableKeys = slices.Clone(c.tableKeys)
	return copied
}

func tomlDuplicateError(name string) error {
	return newValueError(`Key "` + name + `" already exists.`)
}

func tomlRedefinitionError() error {
	return newValueError("Redefinition of an existing table")
}

func (s *tomlScanner) controlError(code byte, context string, offset int) error {
	position := s.save()
	s.pos = min(s.pos+offset, len(s.src))
	defer s.restore(position)
	return s.parseError(fmt.Sprintf(
		"Control characters (codes less than 0x1f and 0x7f) are not allowed in %s, use \\u%04x instead", context, code))
}

func tomlControl(code byte) bool {
	return code == 0x7F || code <= 0x1F && code != '\t'
}

func (s *tomlScanner) charRepr() string {
	if s.end() {
		return `'\x00'`
	}
	character, _ := utf8.DecodeRuneInString(s.src[s.pos:])
	if character >= 0x80 && character <= 0xA0 || character == 0xAD {
		return fmt.Sprintf(`'\x%02x'`, character)
	}
	return jsonx.PythonStrRepr(string(character))
}

type tomlScanner struct {
	src       string
	pos       int
	mark      int
	line      int
	lineStart int
	elided    [][2]int
}

type tomlPosition struct {
	pos, line, lineStart int
}

func (s *tomlScanner) end() bool {
	return s.pos >= len(s.src)
}

func (s *tomlScanner) at(offset int) int {
	if s.pos+offset >= len(s.src) {
		return tomlEOF
	}
	return int(s.src[s.pos+offset])
}

func (s *tomlScanner) cur() int {
	return s.at(0)
}

func (s *tomlScanner) step() {
	if s.end() {
		return
	}
	if s.src[s.pos] == '\n' {
		s.line++
		s.lineStart = s.pos + 1
	}
	s.pos++
}

func (s *tomlScanner) save() tomlPosition {
	return tomlPosition{pos: s.pos, line: s.line, lineStart: s.lineStart}
}

func (s *tomlScanner) restore(position tomlPosition) {
	s.pos, s.line, s.lineStart = position.pos, position.line, position.lineStart
}

func (s *tomlScanner) setMark() {
	s.mark = s.pos
}

func (s *tomlScanner) extract() string {
	return s.src[s.mark:s.pos]
}

func (s *tomlScanner) elidedFrom(start int) int {
	index, _ := slices.BinarySearchFunc(s.elided, start, func(span [2]int, target int) int { return span[0] - target })
	return index
}

// Excludes the whitespace tomlkit drops after dates.
func (s *tomlScanner) rawSlice(start, end int) string {
	index := s.elidedFrom(start)
	if index == len(s.elided) || s.elided[index][0] >= end {
		return s.src[start:end]
	}
	var text strings.Builder
	for ; index < len(s.elided) && s.elided[index][0] < end; index++ {
		text.WriteString(s.src[start:s.elided[index][0]])
		start = s.elided[index][1]
	}
	text.WriteString(s.src[start:end])
	return text.String()
}

func (s *tomlScanner) rawOffset(start, end int) int {
	offset := end - start
	for index := s.elidedFrom(start); index < len(s.elided) && s.elided[index][0] < end; index++ {
		offset -= s.elided[index][1] - s.elided[index][0]
	}
	return offset
}

func (s *tomlScanner) parseError(message string) error {
	line, column := tomlkitLineCol(s.src, s.pos)
	return newValueError(fmt.Sprintf("%s at line %d col %d", message, line, column))
}

func tomlkitLineCol(src string, pos int) (int, int) {
	index := utf8.RuneCountInString(src[:pos])
	lengths := pythonLineLengths(src)
	current := 0
	for line, length := range lengths {
		if current+length+1 > index {
			return line + 1, index - current
		}
		current += length + 1
	}
	return len(lengths), 0
}

func (s *tomlScanner) appendError(err error) error {
	return s.parseError(err.Error())
}

func pythonLineLengths(text string) []int {
	var lengths []int
	length := 0
	runes := []rune(text)
	for index := 0; index < len(runes); index++ {
		switch runes[index] {
		case '\r':
			if index+1 < len(runes) && runes[index+1] == '\n' {
				index++
			}
		case '\n', '\v', '\f', 0x1C, 0x1D, 0x1E, 0x85, 0x2028, 0x2029:
		default:
			length++
			continue
		}
		lengths = append(lengths, length)
		length = 0
	}
	if length > 0 {
		lengths = append(lengths, length)
	}
	return lengths
}

func (s *tomlScanner) unexpected() error {
	return s.parseError("Unexpected character: " + s.charRepr())
}

func (s *tomlScanner) advanceSpaces() {
	for s.cur() == ' ' || s.cur() == '\t' {
		s.step()
	}
}

func parseTOMLContainer(src string) (tomlContainer, error) {
	if !utf8.ValidString(src) {
		return tomlContainer{}, newValueError("TOML documents must be UTF-8")
	}
	s := &tomlScanner{src: src, line: 1}
	var root tomlContainer
	for !s.end() && s.cur() != '[' {
		key, item, ok, err := parseTOMLStatement(s)
		if err != nil {
			return tomlContainer{}, err
		}
		if !ok {
			break
		}
		if key != nil && key.dotted || !mergeTOMLWhitespace(root.body, item) {
			if err := root.append(key, item, true); err != nil {
				return tomlContainer{}, s.appendError(err)
			}
		}
	}
	for !s.end() {
		key, item, err := parseTOMLTable(s, nil, nil)
		if err != nil {
			return tomlContainer{}, err
		}
		if item.kind == tomlTableItem && item.table.aotElement {
			members, err := bundleTOMLAOT(s, item.table, key)
			if err != nil {
				return tomlContainer{}, err
			}
			item = aotItem(members)
		}
		if err := root.append(key, item, true); err != nil {
			return tomlContainer{}, s.appendError(err)
		}
	}
	return root, nil
}

func mergeTOMLWhitespace(body []tomlEntry, item tomlItem) bool {
	if item.kind != tomlWhitespaceItem || len(body) == 0 {
		return false
	}
	last := &body[len(body)-1]
	if last.item.kind != tomlWhitespaceItem {
		return false
	}
	last.item.raw += item.raw
	return true
}

func parseTOMLStatement(s *tomlScanner) (*tomlKey, tomlItem, bool, error) {
	s.setMark()
	for {
		switch s.cur() {
		case tomlEOF, '[':
			return nil, tomlItem{}, false, nil
		case '\n':
			s.step()
			return nil, whitespaceItem(s.extract()), true, nil
		case ' ', '\t':
			s.step()
			if s.end() {
				return nil, whitespaceItem(s.extract()), true, nil
			}
			continue
		case '\r':
			if s.at(1) != '\n' {
				return nil, tomlItem{}, false, s.controlError('\r', "documents", 1)
			}
			s.step()
			continue
		case '#':
			indent := s.extract()
			trivia, err := s.commentTrail(true)
			if err != nil {
				return nil, tomlItem{}, false, err
			}
			trivia.indent = indent
			return nil, commentItem(trivia), true, nil
		}
		break
	}
	indent := s.extract()
	key, item, err := parseTOMLKeyValue(s, true, 0)
	if err != nil {
		return nil, tomlItem{}, false, err
	}
	if trivia := item.valueTrivia(); trivia != nil {
		trivia.indent = indent
	}
	return key, item, true, nil
}

func (s *tomlScanner) commentTrail(parseTrail bool) (tomlTrivia, error) {
	var trivia tomlTrivia
	if s.end() {
		return trivia, nil
	}
	s.setMark()
loop:
	for {
		switch code := s.cur(); code {
		case tomlEOF, '\n':
			break loop
		case '#':
			trivia.commentWS = s.extract()
			s.setMark()
			s.step()
			for !s.end() && s.cur() != '\n' && s.cur() != '\r' {
				if tomlControl(s.src[s.pos]) {
					return trivia, s.controlError(s.src[s.pos], "comments", 0)
				}
				s.step()
			}
			trivia.comment = s.extract()
			s.setMark()
			break loop
		case ' ', '\t':
			s.step()
		case '\r':
			if s.at(1) != '\n' {
				return trivia, s.controlError('\r', "comments", 1)
			}
			s.step()
		default:
			return trivia, s.unexpected()
		}
		if s.end() {
			break
		}
	}
	if parseTrail {
		mark := s.mark
		s.advanceSpaces()
		if s.cur() == '\r' {
			if s.at(1) != '\n' {
				return trivia, s.controlError('\r', "documents", 1)
			}
			s.step()
		}
		if s.cur() == '\n' {
			s.step()
		}
		if s.pos != mark {
			trivia.trail = s.extract()
		}
	}
	return trivia, nil
}

func parseTOMLKeyValue(s *tomlScanner, parseComment bool, depth int) (*tomlKey, tomlItem, error) {
	s.setMark()
	s.advanceSpaces()
	indent := s.extract()
	key, err := parseTOMLKey(s)
	if err != nil {
		return nil, tomlItem{}, err
	}
	s.setMark()
	foundEquals := s.cur() == '='
	for s.cur() == ' ' || s.cur() == '\t' || s.cur() == '=' {
		s.step()
		if s.cur() == '=' {
			if foundEquals {
				return nil, tomlItem{}, s.parseError("Unexpected character: '='")
			}
			foundEquals = true
		}
	}
	if !foundEquals {
		return nil, tomlItem{}, s.unexpected()
	}
	key.sep = s.extract()
	combined := depth + len(key.parts) - 1
	if combined > tomlMaxValueDepth {
		return nil, tomlItem{}, s.parseError(fmt.Sprintf("TOML value nested more than %d levels deep", tomlMaxValueDepth))
	}
	item, err := parseTOMLValue(s, combined)
	if err != nil {
		return nil, tomlItem{}, err
	}
	if trivia := item.valueTrivia(); trivia != nil {
		if parseComment {
			trail, err := s.commentTrail(true)
			if err != nil {
				return nil, tomlItem{}, err
			}
			if trivia.commentWS == "" {
				trivia.commentWS = trail.commentWS
			}
			trivia.comment = trail.comment
			trivia.trail = trail.trail
		} else {
			trivia.trail = ""
		}
		trivia.indent = indent
	}
	return key, item, nil
}

func parseTOMLKey(s *tomlScanner) (*tomlKey, error) {
	first, err := parseTOMLSimpleKey(s)
	if err != nil {
		return nil, err
	}
	parts := []tomlKeyPart{first}
	for s.cur() == '.' {
		if len(parts)+1 > tomlMaxKeyDepth {
			return nil, s.parseError(fmt.Sprintf("TOML key nested more than %d levels deep", tomlMaxKeyDepth))
		}
		s.step()
		part, err := parseTOMLSimpleKey(s)
		if err != nil {
			return nil, err
		}
		parts = append(parts, part)
	}
	return &tomlKey{parts: parts, dotted: len(parts) > 1}, nil
}

func isTOMLBareKeyByte(code int) bool {
	return code >= 'a' && code <= 'z' || code >= 'A' && code <= 'Z' || code >= '0' && code <= '9' || code == '_' || code == '-'
}

func parseTOMLSimpleKey(s *tomlScanner) (tomlKeyPart, error) {
	s.setMark()
	s.advanceSpaces()
	rawStart := s.mark
	if quote := s.cur(); quote == '"' || quote == '\'' {
		if s.at(1) == quote && s.at(2) == quote {
			if _, err := parseTOMLMultilineString(s, s.src[s.pos]); err != nil {
				return tomlKeyPart{}, err
			}
			return tomlKeyPart{}, s.parseError(fmt.Sprintf("Unexpected character: '%c%c%c'", quote, quote, quote))
		}
		name, err := parseTOMLSingleLineString(s, s.src[s.pos])
		if err != nil {
			return tomlKeyPart{}, err
		}
		s.advanceSpaces()
		return tomlKeyPart{name: name, raw: s.src[rawStart:s.pos], quoted: true}, nil
	}
	start := s.pos
	for isTOMLBareKeyByte(s.cur()) || s.cur() == ' ' || s.cur() == '\t' {
		s.step()
	}
	name := strings.Trim(s.src[start:s.pos], " \t")
	if name == "" {
		return tomlKeyPart{}, s.parseError(tomlEmptyKey)
	}
	if strings.ContainsAny(name, " \t") {
		return tomlKeyPart{}, s.parseError(`Invalid key "` + name + `"`)
	}
	return tomlKeyPart{name: name, raw: s.src[rawStart:s.pos]}, nil
}

func parseTOMLValue(s *tomlScanner, depth int) (tomlItem, error) {
	start := s.pos
	switch quote := s.cur(); quote {
	case '"', '\'':
		var text string
		var err error
		if s.at(1) == quote && s.at(2) == quote {
			text, err = parseTOMLMultilineString(s, s.src[s.pos])
		} else {
			text, err = parseTOMLSingleLineString(s, s.src[s.pos])
		}
		if err != nil {
			return tomlItem{}, err
		}
		return tomlItem{kind: tomlValueItem, raw: s.src[start:s.pos], value: String{text: text}, trivia: defaultTrivia()}, nil
	case '[', '{':
		if depth >= tomlMaxValueDepth {
			return tomlItem{}, s.parseError(fmt.Sprintf("TOML value nested more than %d levels deep", tomlMaxValueDepth))
		}
		if quote == '[' {
			return parseTOMLArray(s, depth+1)
		}
		return parseTOMLInlineTable(s, depth+1)
	case tomlEOF:
		return tomlItem{}, s.parseError(`Unexpected character: '\x00'`)
	}
	raw := scanTOMLBareValue(s)
	trivia := defaultTrivia()
	if isTOMLDateShape(raw) {
		return parseTOMLDateTail(s, raw, trivia)
	}
	value, err := parseTOMLBareValue(raw, s)
	if err != nil {
		return tomlItem{}, err
	}
	return tomlItem{kind: tomlValueItem, raw: raw, value: value, trivia: trivia}, nil
}

// Follows tomlkit: whitespace after a bare date's tail stays comment whitespace.
func parseTOMLDateTail(s *tomlScanner, raw string, trivia tomlTrivia) (tomlItem, error) {
	date, ok := parseTOMLDate(raw)
	if !ok {
		return tomlItem{}, s.parseError("Invalid date")
	}
	tailStart := s.pos
	for !s.end() && !strings.ContainsRune("\t\n\r#,]}", rune(s.src[s.pos])) {
		s.step()
	}
	tail := s.src[tailStart:s.pos]
	timePart := strings.TrimRightFunc(tail, unicodex.IsPythonSpace)
	trivia.commentWS = tail[len(timePart):]
	if trivia.commentWS != "" {
		s.elided = append(s.elided, [2]int{tailStart + len(timePart), s.pos})
	}
	if timePart == "" {
		return tomlItem{kind: tomlValueItem, raw: raw, value: date, trivia: trivia}, nil
	}
	full := raw + timePart
	value, ok := parseTOMLTemporal(full)
	if !ok || !tomlDateTimeGrammar(full) {
		return tomlItem{}, s.parseError("Invalid date")
	}
	return tomlItem{kind: tomlValueItem, raw: full, value: value, trivia: trivia}, nil
}

func parseTOMLSingleLineString(s *tomlScanner, quote byte) (string, error) {
	s.step()
	var value strings.Builder
	for {
		code := s.cur()
		switch {
		case code == tomlEOF:
			return "", s.parseError("Unexpected end of file")
		case code == int(quote):
			s.step()
			return value.String(), nil
		case code == '\n':
			return "", s.controlError('\n', "strings", 0)
		case code == '\\' && quote == '"':
			s.step()
			if err := parseTOMLEscape(s, &value); err != nil {
				return "", err
			}
		case tomlControl(s.src[s.pos]):
			return "", s.controlError(s.src[s.pos], "strings", 0)
		default:
			value.WriteByte(s.src[s.pos])
			s.step()
		}
	}
}

func parseTOMLMultilineString(s *tomlScanner, quote byte) (string, error) {
	s.step()
	s.step()
	s.step()
	if s.cur() == '\n' {
		s.step()
	} else if s.cur() == '\r' && s.at(1) == '\n' {
		s.step()
		s.step()
	}
	basic := quote == '"'
	var value strings.Builder
	for {
		code := s.cur()
		switch {
		case code == tomlEOF:
			return "", s.parseError("Unexpected end of file")
		case code == int(quote):
			quotes := 0
			for s.cur() == int(quote) {
				s.step()
				quotes++
			}
			if quotes >= 6 {
				return "", s.parseError(fmt.Sprintf("Invalid character %s in string", s.charRepr()))
			}
			if quotes >= 3 {
				value.WriteString(strings.Repeat(string(quote), quotes-3))
				return value.String(), nil
			}
			value.WriteString(strings.Repeat(string(quote), quotes))
		case basic && code == '\\':
			s.step()
			if isTOMLWhitespace(s.cur()) {
				sawNewline := false
				for isTOMLWhitespace(s.cur()) {
					sawNewline = sawNewline || s.cur() == '\n'
					s.step()
				}
				if s.end() {
					return "", s.parseError("Unexpected end of file")
				}
				if !sawNewline {
					return "", s.parseError(fmt.Sprintf("Invalid character %s in string", s.charRepr()))
				}
				continue
			}
			if err := parseTOMLEscape(s, &value); err != nil {
				return "", err
			}
		case code == '\r':
			if s.at(1) != '\n' {
				return "", s.controlError('\r', "strings", 1)
			}
			value.WriteByte('\r')
			s.step()
		case code != '\n' && tomlControl(s.src[s.pos]):
			return "", s.controlError(s.src[s.pos], "strings", 0)
		default:
			value.WriteByte(s.src[s.pos])
			s.step()
		}
	}
}

func isTOMLWhitespace(code int) bool {
	return code == ' ' || code == '\t' || code == '\n' || code == '\r'
}

func parseTOMLArray(s *tomlScanner, depth int) (tomlItem, error) {
	start := s.pos
	s.step()
	var values []Value
	var items []tomlItem
	var spans [][2]int
	var commas []int
	for {
		if err := skipTOMLArraySpace(s); err != nil {
			return tomlItem{}, err
		}
		switch s.cur() {
		case ']':
			s.step()
			return newTOMLArrayItem(s.rawSlice(start, s.pos), values, items, spans, commas)
		case tomlEOF:
			return tomlItem{}, s.parseError("Unexpected end of file")
		case ',':
			return tomlItem{}, s.parseError("Unexpected character: ','")
		}
		itemStart := s.pos
		item, err := parseTOMLValue(s, depth)
		if err != nil {
			return tomlItem{}, err
		}
		value, err := item.unwrapValue()
		if err != nil {
			return tomlItem{}, err
		}
		values = append(values, value)
		items = append(items, item)
		spans = append(spans, [2]int{s.rawOffset(start, itemStart), s.rawOffset(start, s.pos)})
		if err := skipTOMLArraySpace(s); err != nil {
			return tomlItem{}, err
		}
		switch s.cur() {
		case ',':
			commas = append(commas, s.rawOffset(start, s.pos))
			s.step()
		case ']':
		case tomlEOF:
			return tomlItem{}, s.parseError("Unexpected end of file")
		default:
			return tomlItem{}, s.unexpected()
		}
	}
}

func newTOMLArrayItem(raw string, values []Value, items []tomlItem, spans [][2]int, commas []int) (tomlItem, error) {
	array, err := NewArray(values...)
	if err != nil {
		return tomlItem{}, err
	}
	return tomlItem{kind: tomlArrayItem, raw: raw, value: array, trivia: defaultTrivia(), items: items, spans: spans, commas: commas}, nil
}

func skipTOMLArraySpace(s *tomlScanner) error {
	for {
		switch s.cur() {
		case ' ', '\t', '\n', '\r':
			s.step()
		case '#':
			if _, err := s.commentTrail(false); err != nil {
				return err
			}
		default:
			return nil
		}
	}
}

func parseTOMLInlineTable(s *tomlScanner, depth int) (tomlItem, error) {
	start := s.pos
	s.step()
	var entries []tomlInlineEntry
	expectKey := true
	for {
		if err := skipTOMLArraySpace(s); err != nil {
			return tomlItem{}, err
		}
		if s.cur() == '}' {
			s.step()
			break
		}
		if s.end() {
			return tomlItem{}, s.parseError("Unexpected end of file")
		}
		if expectKey {
			if s.cur() == ',' {
				return tomlItem{}, s.parseError("Unexpected character: ','")
			}
			elided := len(s.elided)
			key, item, err := parseTOMLKeyValue(s, false, depth)
			if err != nil {
				return tomlItem{}, err
			}
			if len(key.parts) > 1 && item.kind == tomlValueItem {
				// tomlkit renders a dotted inline entry with its comment whitespace, so a date keeps its trailing blanks.
				s.elided = s.elided[:elided]
			}
			entries = append(entries, tomlInlineEntry{key: *key, item: item})
			expectKey = false
			continue
		}
		if s.cur() != ',' {
			return tomlItem{}, s.unexpected()
		}
		s.step()
		expectKey = true
	}
	value, err := inlineTableValue(entries)
	if err != nil {
		return tomlItem{}, err
	}
	return tomlItem{kind: tomlInlineItem, raw: s.rawSlice(start, s.pos), value: value, trivia: defaultTrivia(), entries: entries}, nil
}

type inlineNode struct {
	names    []string
	children map[string]*inlineNode
	value    Value
	open     bool
}

func inlineTableValue(entries []tomlInlineEntry) (Value, error) {
	root := &inlineNode{children: map[string]*inlineNode{}, open: true}
	for _, entry := range entries {
		value, err := entry.item.unwrapValue()
		if err != nil {
			return nil, err
		}
		if err := root.assign(entry.key.names(), value); err != nil {
			return nil, err
		}
	}
	return root.object()
}

func (n *inlineNode) assign(names []string, value Value) error {
	node := n
	for _, name := range names[:len(names)-1] {
		child, exists := node.children[name]
		if !exists {
			child = &inlineNode{children: map[string]*inlineNode{}, open: true}
			node.children[name] = child
			node.names = append(node.names, name)
		} else if !child.open {
			return tomlDuplicateError(name)
		}
		node = child
	}
	last := names[len(names)-1]
	if _, exists := node.children[last]; exists {
		return tomlDuplicateError(last)
	}
	node.children[last] = &inlineNode{value: value}
	node.names = append(node.names, last)
	return nil
}

func (n *inlineNode) object() (Value, error) {
	if !n.open {
		return n.value, nil
	}
	members := make([]ObjectMember, 0, len(n.names))
	for _, name := range n.names {
		value, err := n.children[name].object()
		if err != nil {
			return nil, err
		}
		members = append(members, ObjectMember{Key: name, Value: value})
	}
	return NewObject(members...)
}

func parseTOMLTable(s *tomlScanner, parent *tomlKey, aotStack [][]string) (*tomlKey, tomlItem, error) {
	indent := s.src[s.lineStart:s.pos]
	s.step()
	if s.end() {
		return nil, tomlItem{}, s.parseError("Unexpected end of file")
	}
	isAOT := s.cur() == '['
	if isAOT {
		s.step()
	}
	key, err := parseTOMLKey(s)
	var failure *ValueError
	if errors.As(err, &failure) && strings.HasPrefix(failure.Message, tomlEmptyKey+" at line ") {
		return nil, tomlItem{}, s.parseError("Empty table name")
	}
	if err != nil {
		return nil, tomlItem{}, err
	}
	if s.end() {
		return nil, tomlItem{}, s.parseError("Unexpected end of file")
	}
	if s.cur() != ']' {
		return nil, tomlItem{}, s.unexpected()
	}
	key.sep = ""
	fullKey := key.clone()
	fullName := fullKey.asString()
	if slices.ContainsFunc(key.parts, func(part tomlKeyPart) bool { return !part.quoted && strings.Contains(part.name, " ") }) {
		return nil, tomlItem{}, s.parseError(`Invalid table name "` + fullName + `"`)
	}
	parentParts := 0
	if parent != nil {
		parentParts = min(len(parent.parts), len(key.parts))
	}
	relative := slices.Clone(key.parts[parentParts:])
	s.step()
	if isAOT {
		if s.end() {
			return nil, tomlItem{}, s.parseError("Unexpected end of file")
		}
		if s.cur() != ']' {
			return nil, tomlItem{}, s.unexpected()
		}
		s.step()
	}
	trivia, err := s.commentTrail(true)
	if err != nil {
		return nil, tomlItem{}, err
	}
	trivia.indent = indent
	resultKey := fullKey.clone()
	if len(relative) > 0 {
		resultKey = partKey(relative[0], false)
	}
	fullNames := fullKey.names()
	leaf := tomlTable{trivia: trivia, aotElement: isAOT, displayName: fullName}
	if err := parseTOMLTableBody(s, &leaf, fullKey, aotStack); err != nil {
		return nil, tomlItem{}, err
	}
	if len(relative) > 1 {
		outerAOT := isAOT && slices.ContainsFunc(aotStack, func(names []string) bool {
			return len(names) == 1 && names[0] == relative[0].name
		})
		outer := tomlTable{
			trivia:     tomlTrivia{commentWS: trivia.commentWS, comment: trivia.comment, trail: trivia.trail},
			aotElement: outerAOT,
			implicit:   true,
		}
		node := tableItem(leaf)
		if isAOT {
			node = aotItem([]tomlTable{leaf})
		}
		rest := relative[1:]
		for index := len(rest) - 1; index >= 1; index-- {
			level := tomlTable{trivia: trivia, implicit: true}
			level.body.rawAppend(partKey(rest[index], false), node)
			node = tableItem(level)
		}
		outer.body.rawAppend(partKey(rest[0], false), node)
		return resultKey, tableItem(outer), nil
	}
	if isAOT && (len(aotStack) == 0 || !slices.Equal(aotStack[len(aotStack)-1], fullNames)) {
		members, err := bundleTOMLAOT(s, leaf, fullKey)
		if err != nil {
			return nil, tomlItem{}, err
		}
		return resultKey, aotItem(members), nil
	}
	return resultKey, tableItem(leaf), nil
}

func parseTOMLTableBody(s *tomlScanner, table *tomlTable, fullKey *tomlKey, aotStack [][]string) error {
	for !s.end() {
		key, item, ok, err := parseTOMLStatement(s)
		if err != nil {
			return err
		}
		if !ok {
			if s.cur() != '[' {
				return newValueError("internal TOML parse state")
			}
			next, err := peekTOMLTable(s)
			if err != nil {
				return err
			}
			if !isTOMLChild(fullKey, next) {
				break
			}
			childKey, childItem, err := parseTOMLTable(s, fullKey, aotStack)
			if err != nil {
				return err
			}
			if err := table.body.tableAppend(childKey, childItem, true, false); err != nil {
				return err
			}
			continue
		}
		if !mergeTOMLWhitespace(table.body.body, item) {
			if err := table.body.tableAppend(key, item, true, false); err != nil {
				return err
			}
		}
	}
	return table.body.validateAll(true)
}

func peekTOMLTable(s *tomlScanner) (*tomlKey, error) {
	start := s.save()
	defer s.restore(start)
	s.step()
	if s.cur() == '[' {
		s.step()
	}
	return parseTOMLKey(s)
}

func isTOMLChild(parent, child *tomlKey) bool {
	parentNames, childNames := parent.names(), child.names()
	if slices.Equal(parentNames, childNames) {
		return false
	}
	return len(parentNames) <= len(childNames) && slices.Equal(parentNames, childNames[:len(parentNames)])
}

func bundleTOMLAOT(s *tomlScanner, first tomlTable, key *tomlKey) ([]tomlTable, error) {
	names := key.names()
	members := []tomlTable{first}
	for !s.end() {
		start := s.save()
		s.step()
		isAOT := s.cur() == '['
		if isAOT {
			s.step()
		}
		next, err := parseTOMLKey(s)
		s.restore(start)
		if err != nil || !isAOT || !slices.Equal(next.names(), names) {
			break
		}
		_, item, err := parseTOMLTable(s, key, [][]string{names})
		if err != nil {
			return nil, err
		}
		if item.kind != tomlTableItem {
			break
		}
		members = append(members, item.table)
	}
	return members, nil
}
