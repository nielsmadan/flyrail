package flyrail

import (
	"cmp"
	"math"
	"slices"
	"strings"
	"unicode/utf8"
)

const (
	tomlCrossesScalar  = "TOML selector crosses a scalar"
	tomlNoncontiguous  = "a noncontiguous TOML table cannot be an owned unit"
	tomlRepresentation = "TOML table representation changes cannot preserve foreign layout"
)

func (d *tomlDocument) clone() documentBackend {
	return &tomlDocument{source: d.source, newline: d.newline, root: d.root.clone()}
}

// Built on first use and dropped on publish.
func (d *tomlDocument) document() (*tkContainer, error) {
	if d.tree == nil {
		tree, err := tkFromContainer(&d.root, false, 0)
		if err != nil {
			return nil, err
		}
		d.tree = tree
	}
	return d.tree, nil
}

func (d *tomlDocument) currentText() string {
	if d.tree != nil {
		return d.tree.asString()
	}
	return d.source
}

func (d *tomlDocument) reload(text string) error {
	parsed, err := parseTOMLDocument(text)
	if err != nil {
		return err
	}
	d.root, d.source, d.tree = parsed.root, parsed.source, nil
	return nil
}

func tkNavigate(root *tkContainer, path []pathPart) (tkNode, error) {
	node := tkNode{root: root}
	for _, part := range path {
		if !part.isIndex {
			mapping := node.mapping()
			if mapping == nil {
				return tkNode{}, newValueError(tomlCrossesScalar)
			}
			next, err := mapping.item(part.key)
			if err != nil {
				return tkNode{}, err
			}
			node = next
			continue
		}
		var members []*tkItem
		switch {
		case node.item != nil && node.item.kind == tomlArrayItem:
			members = node.item.items
		case node.item != nil && node.item.kind == tomlAOTItem:
			members = node.item.aot
		default:
			return tkNode{}, newValueError(tomlCrossesScalar)
		}
		if part.index < 0 || part.index >= len(members) {
			return tkNode{}, tkMissing()
		}
		node = tkNode{item: members[part.index]}
	}
	return node, nil
}

func (d *tomlDocument) node(path []pathPart) (tkNode, error) {
	tree, err := d.document()
	if err != nil {
		return tkNode{}, err
	}
	return tkNavigate(tree, path)
}

func tkTables(node tkNode) []*tkItem {
	if node.proxy != nil {
		return node.proxy.tables
	}
	return []*tkItem{node.item}
}

func tkCopy(node tkNode) (tkNode, error) {
	if node.proxy == nil {
		return tkNode{item: node.item.clone()}, nil
	}
	container := &tkContainer{parsed: true}
	for _, table := range node.proxy.tables {
		if err := container.rawAppend(tkSingleKey("value"), table.clone()); err != nil {
			return tkNode{}, err
		}
	}
	return container.item("value")
}

func tkInvalidate(item *tkItem, depth int) {
	if depth > tkMaxDepth {
		return
	}
	switch item.kind {
	case tomlTableItem:
		item.displayName = nil
		for _, entry := range item.body.body {
			tkInvalidate(entry.item, depth+1)
		}
	case tomlAOTItem:
		for _, member := range item.aot {
			tkInvalidate(member, depth+1)
		}
	}
}

func tkInvalidateNode(node tkNode) {
	for _, table := range tkTables(node) {
		tkInvalidate(table, 0)
	}
}

func tkTrailing(item *tkItem) *tkContainer {
	for depth := 0; depth <= tkMaxDepth; depth++ {
		switch item.kind {
		case tomlAOTItem:
			if len(item.aot) == 0 {
				return nil
			}
			item = item.aot[len(item.aot)-1]
		case tomlTableItem:
			body := item.body.body
			if len(body) == 0 || !tkTableLike(body[len(body)-1].item) {
				return item.body
			}
			item = body[len(body)-1].item
		default:
			return nil
		}
	}
	return nil
}

func tkTrailingNode(node tkNode) *tkContainer {
	tables := tkTables(node)
	if len(tables) == 0 {
		return nil
	}
	if node.proxy != nil {
		return tkTrailing(tables[len(tables)-1])
	}
	return tkTrailing(node.item)
}

func tkStripGap(node tkNode) string {
	container := tkTrailingNode(node)
	gap := ""
	for container != nil && len(container.body) > 0 {
		last := container.body[len(container.body)-1].item
		if last.kind != tomlCommentItem && last.kind != tomlWhitespaceItem && last.kind != tomlNullItem {
			break
		}
		container.body = container.body[:len(container.body)-1]
		gap = last.asString() + gap
	}
	return gap
}

func tkAppendGap(node tkNode, gap string) {
	if container := tkTrailingNode(node); gap != "" && container != nil {
		container.body = append(container.body, tkEntry{item: tkWhitespace(gap, true)})
	}
}

func tkPreserveHeader(node, current tkNode) {
	tables := tkTables(current)
	live := slices.IndexFunc(tables, func(table *tkItem) bool { return !table.isSuperTable() })
	if live < 0 {
		return
	}
	targets := tkTables(node)
	if len(targets) == 0 {
		return
	}
	header := targets[0]
	if index := slices.IndexFunc(targets, func(table *tkItem) bool { return !table.isSuperTable() }); index >= 0 {
		header = targets[index]
	}
	header.super = tkSuperFalse
	header.trivia = tables[live].trivia
	header.displayName = nil
	if name := tables[live].displayName; name != nil {
		header.displayName = stringRef(*name)
	}
}

func hasPathPrefix(path, prefix []pathPart) bool {
	return len(path) >= len(prefix) && slices.EqualFunc(path[:len(prefix)], prefix, partsMatch)
}

func (d *tomlDocument) contiguous(path []pathPart) error {
	tree, err := d.document()
	if err != nil {
		return err
	}
	var selected []int
	events := 0
	var visit func(item *tkItem, current []pathPart, depth int)
	visit = func(item *tkItem, current []pathPart, depth int) {
		if depth > tkMaxDepth {
			return
		}
		event := func() {
			if hasPathPrefix(current, path) {
				selected = append(selected, events)
			}
			events++
		}
		switch item.kind {
		case tomlTableItem:
			if !item.isSuperTable() {
				event()
			}
			for _, entry := range item.body.body {
				if entry.key != nil {
					visit(entry.item, appendPath(current, segmentKey(entry.key.name())), depth+1)
				}
			}
		case tomlAOTItem:
			for index, member := range item.aot {
				visit(member, appendPath(current, segmentIndex(index)), depth+1)
			}
		default:
			event()
		}
	}
	for _, entry := range tree.body {
		if entry.key != nil {
			visit(entry.item, []pathPart{segmentKey(entry.key.name())}, 0)
		}
	}
	if len(selected) > 0 && selected[len(selected)-1]-selected[0] != len(selected)-1 {
		return newValueError(tomlNoncontiguous)
	}
	return nil
}

func (d *tomlDocument) fragmentSyntax(node tkNode, synthetic bool) []byte {
	document := &tkContainer{}
	for _, table := range node.proxy.tables {
		if !table.isSuperTable() {
			table.trivia = tomlTrivia{trail: d.newline}
		}
		if synthetic {
			tkInvalidate(table, 0)
		} else {
			table.displayName = nil
		}
		document.body = append(document.body, tkEntry{key: tkSingleKey("value"), item: table})
	}
	return []byte(document.asString())
}

func (d *tomlDocument) selectedCopy(path []pathPart) (tkNode, error) {
	located, err := d.node(path)
	if err != nil {
		return tkNode{}, err
	}
	node, err := tkCopy(located)
	if err != nil {
		return tkNode{}, err
	}
	if node.tableLike() {
		if err := d.contiguous(path); err != nil {
			return tkNode{}, err
		}
	}
	tkStripGap(node)
	return node, nil
}

func (d *tomlDocument) ownedSyntax(path []pathPart) ([]byte, error) {
	node, err := d.selectedCopy(path)
	if err != nil {
		return nil, err
	}
	switch {
	case node.proxy != nil:
		return d.fragmentSyntax(node, false), nil
	case node.item.kind == tomlAOTItem:
		document := &tkContainer{body: []tkEntry{{key: tkSingleKey("value"), item: node.item}}}
		return []byte(document.asString()), nil
	}
	return []byte(node.item.asString()), nil
}

func finiteTOMLValue(value Value, depth int) error {
	if depth > tkMaxDepth {
		return newNestingError(documentNesting)
	}
	switch v := value.(type) {
	case Float:
		if number := v.Float64(); math.IsInf(number, 0) || math.IsNaN(number) {
			return newValueError("floats must be finite")
		}
	case Array:
		for _, item := range v.items {
			if err := finiteTOMLValue(item, depth+1); err != nil {
				return err
			}
		}
	case Object:
		for _, member := range v.members {
			if err := finiteTOMLValue(member.Value, depth+1); err != nil {
				return err
			}
		}
	}
	return nil
}

func (d *tomlDocument) syntax(path []pathPart) ([]byte, error) {
	value, err := d.native(path)
	if err != nil {
		return nil, err
	}
	if err := finiteTOMLValue(value, 0); err != nil {
		return nil, err
	}
	node, err := d.selectedCopy(path)
	if err != nil {
		return nil, err
	}
	if node.proxy != nil {
		return d.fragmentSyntax(node, true), nil
	}
	node.item.trivia = tomlTrivia{trail: d.newline}
	tkInvalidate(node.item, 0)
	document := &tkContainer{}
	if err := document.setItem("value", node.item); err != nil {
		return nil, err
	}
	return []byte(document.asString()), nil
}

func (d *tomlDocument) emptySyntax(path []pathPart) ([]byte, error) {
	node, err := d.node(path)
	if err != nil {
		return nil, err
	}
	if node.proxy != nil {
		return nil, newValueError("a noncontiguous TOML table cannot be pruned")
	}
	if node.root != nil {
		return []byte(node.root.asString()), nil
	}
	raw := node.item.asString()
	if node.item.kind == tomlTableItem || node.item.kind == tomlInlineItem {
		raw += node.item.trivia.commentWS + node.item.trivia.comment
	}
	return []byte(raw), nil
}

func (d *tomlDocument) arrayParts(path []pathPart) (*tkItem, error) {
	node, err := d.node(path)
	if err != nil {
		return nil, err
	}
	if node.item == nil || node.item.kind != tomlArrayItem {
		return nil, newValueError("inline array edit requires an array")
	}
	return node.item, nil
}

func (d *tomlDocument) positionPrefix(path []pathPart) ([]byte, error) {
	if len(path) == 0 {
		return []byte{}, nil
	}
	parentPath, last := path[:len(path)-1], path[len(path)-1]
	parent, err := d.node(parentPath)
	if err != nil {
		return nil, err
	}
	if parent.item == nil || parent.item.kind != tomlArrayItem {
		return []byte{}, nil
	}
	if !last.isIndex {
		return nil, newValueError("array position requires a member")
	}
	array := parent.item
	index := last.index
	if index < 0 || index >= len(array.spans) || index > 0 && index-1 >= len(array.commas) {
		return nil, tkMissing()
	}
	start := 1
	if index > 0 {
		start = array.commas[index-1] + 1
	}
	if start > array.spans[index][0] {
		return []byte{}, nil
	}
	return []byte(array.raw[start:array.spans[index][0]]), nil
}

func (d *tomlDocument) publish() error {
	tree, err := d.document()
	if err != nil {
		return err
	}
	expected, err := tree.unwrap(0)
	if err != nil {
		return err
	}
	parsed, err := parseTOMLDocument(tree.asString())
	if err != nil {
		if failure, ok := asValueError(err); ok && failure.nesting {
			return err
		}
		return newValueError("TOML layout cannot preserve valid syntax")
	}
	actual, err := unwrapTOMLBody(&parsed.root)
	if err != nil {
		return newValueError("TOML layout cannot preserve valid syntax")
	}
	if !tkSemanticEqual(expected, actual) {
		return newValueError("TOML layout cannot preserve the selected and foreign values")
	}
	d.root, d.source, d.tree = parsed.root, parsed.source, nil
	return nil
}

func (d *tomlDocument) restoreLayout(path []pathPart, layout []byte) error {
	headers, err := layoutHeaders(layout)
	if err != nil {
		return err
	}
	for _, header := range headers {
		var expected []string
		target := slices.Clone(path)
		for _, part := range path {
			if !part.isIndex {
				expected = append(expected, part.key)
			}
		}
		for _, part := range header.path {
			if part.isIndex {
				if part.index > math.MaxInt32 {
					return newValueError("invalid TOML restoration path")
				}
				target = append(target, segmentIndex(int(part.index)))
				continue
			}
			expected = append(expected, part.field)
			target = append(target, segmentKey(part.field))
		}
		names, err := headerPath(header.name)
		if err != nil {
			return err
		}
		if !slices.Equal(names, expected) {
			return newValueError("TOML restoration header does not match its destination")
		}
		node, err := d.node(target)
		if err != nil {
			if isTkMissing(err) {
				return newValueError("invalid TOML restoration path")
			}
			return err
		}
		var table *tkItem
		if node.proxy != nil {
			index := slices.IndexFunc(node.proxy.tables, func(item *tkItem) bool { return !item.isSuperTable() })
			if index >= 0 {
				table = node.proxy.tables[index]
			}
		} else if node.item != nil && node.item.kind == tomlTableItem {
			table = node.item
		}
		if table == nil {
			return invalidHeader()
		}
		table.displayName = stringRef(header.name)
	}
	return nil
}

func (d *tomlDocument) generated(value Value, syntax []byte, element bool) (tkNode, error) {
	var text string
	if syntax != nil {
		if !utf8.Valid(syntax) {
			return tkNode{}, newValueError("TOML documents must be UTF-8")
		}
		text = string(syntax)
	} else {
		item, err := tkGenerate(tkNativeOf(value), tkParentNone, 0)
		if err != nil {
			return tkNode{}, err
		}
		document := &tkContainer{}
		if err := document.setItem("value", item); err != nil {
			return tkNode{}, err
		}
		text = strings.ReplaceAll(document.asString(), "\n", d.newline)
	}
	parsed, err := parseTOMLContainer(text)
	if err != nil {
		return tkNode{}, err
	}
	tree, err := tkFromContainer(&parsed, false, 0)
	if err != nil {
		return tkNode{}, err
	}
	node, err := tree.item("value")
	if err != nil {
		return tkNode{}, err
	}
	if element && node.item != nil && node.item.kind == tomlAOTItem && len(node.item.aot) == 1 {
		node = tkNode{item: node.item.aot[0]}
	}
	tkInvalidateNode(node)
	return node, nil
}

func (d *tomlDocument) put(path []pathPart, value Value, syntax, layout []byte) error {
	if err := d.putPath(path, value, syntax); err != nil {
		return err
	}
	if err := d.restoreLayout(path, layout); err != nil {
		return err
	}
	return d.publish()
}

func (d *tomlDocument) dotted(path []pathPart) (bool, error) {
	if len(path) == 0 || path[len(path)-1].isIndex {
		return false, nil
	}
	parent, err := d.node(path[:len(path)-1])
	if err != nil {
		return false, err
	}
	container := parent.mapping()
	if container == nil || parent.item != nil && parent.item.kind != tomlTableItem && parent.item.kind != tomlInlineItem {
		return false, nil
	}
	name := path[len(path)-1].key
	return slices.ContainsFunc(container.body, func(entry tkEntry) bool {
		return entry.key != nil && entry.key.name() == name && entry.key.dotted
	}), nil
}

type tkTriviaRecord struct {
	item   *tkItem
	trivia tomlTrivia
}

func tkCollectTrivia(item *tkItem, records *[]tkTriviaRecord, depth int) {
	if depth > tkMaxDepth {
		return
	}
	if item.kind != tomlWhitespaceItem {
		*records = append(*records, tkTriviaRecord{item: item, trivia: item.trivia})
	}
	switch item.kind {
	case tomlTableItem, tomlInlineItem:
		for _, entry := range item.body.body {
			tkCollectTrivia(entry.item, records, depth+1)
		}
	case tomlArrayItem:
		for _, member := range item.items {
			tkCollectTrivia(member, records, depth+1)
		}
	case tomlAOTItem:
		for _, member := range item.aot {
			tkCollectTrivia(member, records, depth+1)
		}
	}
}

func setOnNode(parent tkNode, name string, value *tkItem) error {
	switch {
	case parent.root != nil:
		return parent.root.setItem(name, value)
	case parent.proxy != nil:
		return parent.proxy.setItem(name, value, 0)
	}
	return parent.item.tableSetItem(name, value)
}

func delOnNode(parent tkNode, name string) error {
	if parent.proxy != nil {
		return parent.proxy.delItem(name)
	}
	return parent.mapping().remove(name)
}

func (d *tomlDocument) putPath(path []pathPart, value Value, syntax []byte) error {
	if len(path) == 0 {
		return newValueError(tomlCrossesScalar)
	}
	parentPath, part := path[:len(path)-1], path[len(path)-1]
	generated, err := d.generated(value, syntax, part.isIndex)
	if err != nil {
		return err
	}
	parent, err := d.node(parentPath)
	if err != nil {
		return err
	}
	var current *tkNode
	if !part.isIndex && parent.mapping() != nil && parent.mapping().contains(part.key) {
		located, err := d.node(path)
		if err != nil {
			return err
		}
		current = &located
	}
	if current != nil && current.proxy != nil || generated.proxy != nil {
		return d.putFragments(path, generated, current)
	}
	node := generated.item
	if !part.isIndex && parent.mapping() != nil && !parent.mapping().contains(part.key) {
		if text := d.currentText(); text != "" && !strings.HasSuffix(text, "\r") && !strings.HasSuffix(text, "\n") {
			if err := d.reload(text + d.newline); err != nil {
				return err
			}
			if parent, err = d.node(parentPath); err != nil {
				return err
			}
		}
	}
	switch {
	case !part.isIndex && parent.mapping() != nil:
		return d.assign(path, parent, node, syntax)
	case part.isIndex && parent.item != nil && parent.item.kind == tomlArrayItem:
		inline, err := tkInline(node)
		if err != nil {
			return err
		}
		raw := inline.asString()
		return d.arrayEdit(parentPath, part.index, &raw, true, nil)
	case part.isIndex && parent.item != nil && parent.item.kind == tomlAOTItem:
		aot := parent.item
		if part.index < 0 || part.index >= len(aot.aot) {
			return tkMissing()
		}
		if node.kind != tomlTableItem {
			return newValueError("TOML member selector requires an array")
		}
		tkAppendGap(tkNode{item: node}, tkStripGap(tkNode{item: aot.aot[part.index].clone()}))
		node.trivia = aot.aot[part.index].trivia
		aot.aot[part.index] = node
		return nil
	}
	return newValueError(tomlCrossesScalar)
}

func (d *tomlDocument) assign(path []pathPart, parent tkNode, node *tkItem, syntax []byte) error {
	name := path[len(path)-1].key
	tree, err := d.document()
	if err != nil {
		return err
	}
	var records []tkTriviaRecord
	for _, entry := range tree.body {
		tkCollectTrivia(entry.item, &records, 0)
	}
	indent := node.trivia.indent
	if parent.mapping().contains(name) {
		located, err := parent.mapping().item(name)
		if err != nil {
			return err
		}
		current := located.item
		if current == nil {
			return newValueError(tomlRepresentation)
		}
		dotted, err := d.dotted(path)
		if err != nil {
			return err
		}
		switch {
		case tkTableLike(current) && node.kind != current.kind:
			return newValueError(tomlRepresentation)
		case !tkTableLike(current):
			if node, err = tkInline(node); err != nil {
				return err
			}
		case node.kind == tomlTableItem && dotted:
			node.super = tkSuperTrue
			if syntax == nil {
				for _, key := range node.body.names() {
					child, err := node.body.item(key)
					if err != nil {
						return err
					}
					if child.item == nil || child.item.kind != tomlTableItem {
						continue
					}
					inline, err := tkInline(child.item)
					if err != nil {
						return err
					}
					if err := node.tableSetItem(key, inline); err != nil {
						return err
					}
				}
				tkStripGap(tkNode{item: node})
			}
		}
		if current.kind == tomlTableItem && node.kind == tomlTableItem {
			tkPreserveHeader(tkNode{item: node}, located)
		}
		tkAppendGap(tkNode{item: node}, tkStripGap(tkNode{item: current.clone()}))
		indent = current.trivia.indent
		node.trivia = current.trivia
	} else {
		dotted, err := d.dotted(path[:len(path)-1])
		if err != nil {
			return err
		}
		if dotted {
			if node, err = tkInline(node); err != nil {
				return err
			}
		}
	}
	bodyLength := 0
	if node.kind == tomlTableItem {
		bodyLength = len(node.body.body)
	}
	var firstIndent *string
	if node.kind == tomlAOTItem && len(node.aot) > 0 {
		firstIndent = stringRef(node.aot[0].trivia.indent)
	}
	if err := setOnNode(parent, name, node); err != nil {
		return err
	}
	if firstIndent != nil {
		node.aot[0].trivia.indent = *firstIndent
	}
	if node.kind == tomlTableItem {
		for len(node.body.body) > bodyLength && node.body.body[len(node.body.body)-1].item.kind == tomlWhitespaceItem {
			node.body.body = node.body.body[:len(node.body.body)-1]
		}
	}
	for _, record := range records {
		record.item.trivia = record.trivia
	}
	node.trivia.indent = indent
	return nil
}

func (d *tomlDocument) physicalParent(path []pathPart) (*tkContainer, error) {
	tree, err := d.document()
	if err != nil {
		return nil, err
	}
	type physical struct {
		container *tkContainer
		item      *tkItem
	}
	parents := []physical{{container: tree}}
	for _, part := range path[:len(path)-1] {
		var children []physical
		for _, parent := range parents {
			body := parent.container
			if parent.item != nil && parent.item.kind == tomlTableItem {
				body = parent.item.body
			}
			switch {
			case body != nil && !part.isIndex:
				for _, entry := range body.body {
					if entry.key != nil && entry.key.name() == part.key {
						children = append(children, physical{item: entry.item})
					}
				}
			case parent.item != nil && parent.item.kind == tomlAOTItem && part.isIndex:
				if part.index < 0 || part.index >= len(parent.item.aot) {
					return nil, tkMissing()
				}
				children = append(children, physical{item: parent.item.aot[part.index]})
			}
		}
		parents = children
	}
	name := path[len(path)-1].key
	var containers []*tkContainer
	for _, parent := range parents {
		body := parent.container
		if parent.item != nil && parent.item.kind == tomlTableItem {
			body = parent.item.body
		}
		if body != nil && body.contains(name) {
			containers = append(containers, body)
		}
	}
	if len(containers) != 1 {
		return nil, newValueError("TOML fragments cross separate parent table representations")
	}
	return containers[0], nil
}

func (d *tomlDocument) putFragments(path []pathPart, node tkNode, current *tkNode) error {
	isTable := func(n tkNode) bool { return n.proxy != nil || n.item != nil && n.item.kind == tomlTableItem }
	if current == nil || !isTable(*current) || !isTable(node) {
		return newValueError(tomlRepresentation)
	}
	if err := d.contiguous(path); err != nil {
		return err
	}
	container, err := d.physicalParent(path)
	if err != nil {
		return err
	}
	tables := tkTables(node)
	tkPreserveHeader(node, *current)
	copied, err := tkCopy(*current)
	if err != nil {
		return err
	}
	tkAppendGap(node, tkStripGap(copied))
	name := path[len(path)-1].key
	var indices []int
	for index, entry := range container.body {
		if entry.key != nil && entry.key.name() == name {
			indices = append(indices, index)
		}
	}
	key := container.body[indices[0]].key
	replacement := make([]tkEntry, 0, len(tables))
	for _, table := range tables {
		replacement = append(replacement, tkEntry{key: key, item: table})
	}
	body := slices.Concat(container.body[:indices[0]], replacement, container.body[indices[len(indices)-1]+1:])
	container.body = nil
	for _, entry := range body {
		if err := container.rawAppend(entry.key, entry.item); err != nil {
			return err
		}
	}
	return nil
}

func (d *tomlDocument) insert(path []pathPart, index int, value Value, syntax, prefix, layout []byte) error {
	parent, err := d.node(path)
	if err != nil {
		return err
	}
	node, err := d.generated(value, syntax, true)
	if err != nil {
		return err
	}
	if node.proxy != nil {
		return newValueError("TOML table fragments cannot become an array member")
	}
	switch {
	case parent.item != nil && parent.item.kind == tomlArrayItem:
		inline, err := tkInline(node.item)
		if err != nil {
			return err
		}
		raw := inline.asString()
		if err := d.arrayEdit(path, index, &raw, false, prefix); err != nil {
			return err
		}
	case parent.item != nil && parent.item.kind == tomlAOTItem && node.item.kind == tomlTableItem:
		parent.item.aotInsert(index, node.item)
	default:
		return newValueError("TOML member selector requires an array")
	}
	if err := d.restoreLayout(appendPath(path, segmentIndex(index)), layout); err != nil {
		return err
	}
	return d.publish()
}

func (d *tomlDocument) relocate(path []pathPart, index int, value Value, syntax, prefix, layout []byte) error {
	if len(path) == 0 {
		return newValueError(tomlCrossesScalar)
	}
	parentPath, part := path[:len(path)-1], path[len(path)-1]
	parent, err := d.node(parentPath)
	if err != nil {
		return err
	}
	if parent.item != nil && parent.item.kind == tomlAOTItem && part.isIndex {
		aot := parent.item
		if part.index < 0 || part.index >= len(aot.aot) {
			return tkMissing()
		}
		member := aot.aot[part.index]
		aot.aot = slices.Delete(aot.aot, part.index, part.index+1)
		aot.aotInsert(index, member)
		return d.put(appendPath(parentPath, segmentIndex(index)), value, syntax, layout)
	}
	if err := d.delete(path); err != nil {
		return err
	}
	return d.insert(parentPath, index, value, syntax, prefix, layout)
}

type tomlPatch struct {
	start, end  int
	replacement string
}

func tomlPrefixed(text string, start, end int, prefix []byte) (int, error) {
	if start > end || !strings.HasPrefix(text[start:end], string(prefix)) {
		return 0, newValueError("original array position trivia changed")
	}
	return start + len(prefix), nil
}

func (d *tomlDocument) arrayEdit(path []pathPart, index int, raw *string, replace bool, prefix []byte) error {
	array, err := d.arrayParts(path)
	if err != nil {
		return err
	}
	text, spans, commas := array.raw, array.spans, array.commas
	var patches []tomlPatch
	switch {
	case raw == nil || replace:
		if index < 0 || index >= len(spans) {
			return tkMissing()
		}
		replacement := ""
		if raw != nil {
			replacement = *raw
		}
		patches = append(patches, tomlPatch{spans[index][0], spans[index][1], replacement})
		if raw == nil && len(commas) > 0 {
			comma := commas[min(index, len(commas)-1)]
			patches = append(patches, tomlPatch{comma, comma + 1, ""})
		}
	case index < len(spans):
		start := 1
		if index > 0 {
			if index-1 >= len(commas) {
				return tkMissing()
			}
			start = commas[index-1] + 1
		}
		if start, err = tomlPrefixed(text, start, spans[index][0], prefix); err != nil {
			return err
		}
		patches = append(patches, tomlPatch{start, start, *raw + ","})
	default:
		start := len(text) - 1
		if prefix != nil {
			boundary := 1
			switch {
			case len(commas) > 0 && len(commas) == len(spans):
				boundary = commas[len(commas)-1] + 1
			case len(spans) > 0:
				boundary = spans[len(spans)-1][1]
			}
			if start, err = tomlPrefixed(text, boundary, start, prefix); err != nil {
				return err
			}
		}
		patches = append(patches, tomlPatch{start, start, *raw})
		if len(spans) > 0 && len(commas) < len(spans) {
			end := spans[len(spans)-1][1]
			patches = append(patches, tomlPatch{end, end, ","})
		} else if len(commas) > 0 {
			patches[0] = tomlPatch{start, start, *raw + ","}
		}
	}
	slices.SortStableFunc(patches, func(a, b tomlPatch) int {
		return cmp.Or(cmp.Compare(b.start, a.start), cmp.Compare(b.end, a.end))
	})
	for _, patch := range patches {
		text = text[:patch.start] + patch.replacement + text[patch.end:]
	}
	return d.put(path, nil, []byte("value = "+text), []byte{})
}

func tkHeaderComment(table *tkItem) string {
	trivia := table.trivia
	if trivia.comment != "" {
		return trivia.indent + trivia.commentWS + trivia.comment + trivia.trail
	}
	return trivia.indent
}

func (d *tomlDocument) delete(path []pathPart) error {
	if len(path) == 0 {
		return newValueError(tomlCrossesScalar)
	}
	parentPath, part := path[:len(path)-1], path[len(path)-1]
	parent, err := d.node(parentPath)
	if err != nil {
		return err
	}
	switch {
	case part.isIndex && parent.item != nil && parent.item.kind == tomlArrayItem:
		if err := d.arrayEdit(parentPath, part.index, nil, false, nil); err != nil {
			return err
		}
	case part.isIndex && parent.item != nil && parent.item.kind == tomlAOTItem:
		if err := d.deleteMember(parentPath, parent.item, part.index); err != nil {
			return err
		}
	case !part.isIndex && parent.mapping() != nil:
		if err := d.deleteKey(path, parent); err != nil {
			return err
		}
	default:
		return newValueError(tomlCrossesScalar)
	}
	return d.publish()
}

func (d *tomlDocument) deleteMember(parentPath []pathPart, aot *tkItem, index int) error {
	if index < 0 || index >= len(aot.aot) {
		return tkMissing()
	}
	table := aot.aot[index]
	comment := tkHeaderComment(table) + tkStripGap(tkNode{item: table.clone()})
	aot.aot = slices.Delete(aot.aot, index, index+1)
	if comment != "" {
		if index < len(aot.aot) {
			aot.aot[index].trivia.indent = comment + aot.aot[index].trivia.indent
		} else if len(aot.aot) > 0 {
			last := aot.aot[len(aot.aot)-1]
			last.body.body = append(last.body.body, tkEntry{item: tkWhitespace(comment, true)})
		}
	}
	if len(aot.aot) == 0 {
		return d.removeEmptyAOT(parentPath, comment)
	}
	return nil
}

func (d *tomlDocument) deleteKey(path []pathPart, parent tkNode) error {
	name := path[len(path)-1].key
	node, err := parent.mapping().item(name)
	if err != nil {
		return err
	}
	if node.proxy != nil && len(node.proxy.tables) == 0 {
		return newValueError("cannot safely detach this TOML comment")
	}
	if node.proxy != nil {
		container, err := d.physicalParent(path)
		if err != nil {
			return err
		}
		parent = tkNode{root: container}
	}
	var body *tkContainer
	switch {
	case parent.root != nil:
		body = parent.root
	case parent.item != nil:
		body = parent.item.body
	}
	retained, retainedGap := -1, ""
	copied, err := tkCopy(node)
	if err != nil {
		return err
	}
	gap := tkStripGap(copied)
	header := node.item
	if node.proxy != nil {
		tables := node.proxy.tables
		header = tables[0]
		if index := slices.IndexFunc(tables, func(table *tkItem) bool { return !table.isSuperTable() }); index >= 0 {
			header = tables[index]
		}
	}
	if trivia := header.trivia; header.kind != tomlAOTItem && trivia.comment != "" {
		gap = trivia.indent + trivia.commentWS + trivia.comment + trivia.trail + gap
	}
	if gap != "" {
		if body == nil {
			return newValueError("cannot safely detach this TOML comment")
		}
		first := node.item
		if node.proxy != nil {
			first = node.proxy.tables[0]
		}
		retained = slices.IndexFunc(body.body, func(entry tkEntry) bool { return entry.item == first })
		if retained < 0 {
			return newValueError("cannot safely detach this TOML comment")
		}
		retainedGap = gap
	}
	if err := delOnNode(parent, name); err != nil {
		return err
	}
	if retained >= 0 {
		body.body[retained] = tkEntry{item: tkWhitespace(retainedGap, true)}
	}
	tree, err := d.document()
	if err != nil {
		return err
	}
	tkRetainEmptyTables(tree, d.newline, 0)
	return nil
}

func tkRetainEmptyTables(container *tkContainer, newline string, depth int) {
	if depth > tkMaxDepth {
		return
	}
	for _, entry := range container.body {
		if entry.item.kind != tomlTableItem {
			continue
		}
		if entry.item.body.length() == 0 && entry.item.isSuperTable() {
			entry.item.super = tkSuperFalse
			entry.item.trivia = tomlTrivia{trail: newline}
		}
		tkRetainEmptyTables(entry.item.body, newline, depth+1)
	}
}

func (d *tomlDocument) removeEmptyAOT(path []pathPart, comment string) error {
	parent, err := d.node(path[:len(path)-1])
	if err != nil {
		return err
	}
	var container *tkContainer
	switch {
	case parent.root != nil:
		container = parent.root
	case parent.item != nil && (parent.item.kind == tomlTableItem || parent.item.kind == tomlInlineItem):
		container = parent.item.body
	default:
		return newValueError("cannot safely detach this TOML array header comment")
	}
	name := path[len(path)-1].key
	index := slices.IndexFunc(container.body, func(entry tkEntry) bool { return entry.key != nil && entry.key.name() == name })
	if err := container.remove(name); err != nil {
		return err
	}
	if comment != "" {
		container.body[index] = tkEntry{item: tkWhitespace(comment, true)}
	}
	return nil
}
