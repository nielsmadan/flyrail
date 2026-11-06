package flyrail

import "strings"

func (i *tomlItem) asString() string {
	switch i.kind {
	case tomlValueItem, tomlArrayItem, tomlInlineItem, tomlWhitespaceItem:
		return i.raw
	case tomlCommentItem:
		return i.trivia.indent + i.trivia.commentWS + i.trivia.comment + i.trivia.trail
	case tomlTableItem:
		return renderTOMLTableBody(&i.table)
	case tomlAOTItem:
		var text strings.Builder
		for index := range i.aot {
			text.WriteString(renderTOMLTableBody(&i.aot[index]))
		}
		return text.String()
	}
	return ""
}

func prefixedTOMLName(prefix *string, name string) string {
	if prefix != nil && name != "" {
		return *prefix + "." + name
	}
	return name
}

func renderTOMLSimple(key *tomlKey, item *tomlItem, prefix *string) string {
	trivia := item.valueTrivia()
	if key == nil || trivia == nil {
		return item.asString()
	}
	return trivia.indent + prefixedTOMLName(prefix, key.asString()) + key.sep + item.asString() +
		trivia.commentWS + trivia.comment + trivia.trail
}

func needsTOMLSeparator(text string, indent string) bool {
	trimmed := strings.Trim(text, " ")
	return trimmed != "" && !strings.HasSuffix(trimmed, "\n") && !strings.Contains(indent, "\n")
}

func firstTOMLIndent(members []tomlTable) string {
	if len(members) == 0 {
		return ""
	}
	return members[0].trivia.indent
}

func renderTOMLEntries(text *strings.Builder, body []tomlEntry, tablePrefix func(entry *tomlEntry) *string, aotPrefix, simplePrefix *string) {
	for index := range body {
		entry := &body[index]
		switch entry.item.kind {
		case tomlTableItem:
			if needsTOMLSeparator(text.String(), entry.item.table.trivia.indent) {
				text.WriteByte('\n')
			}
			text.WriteString(renderTOMLTable(entry.key, &entry.item.table, tablePrefix(entry)))
		case tomlAOTItem:
			if needsTOMLSeparator(text.String(), firstTOMLIndent(entry.item.aot)) {
				text.WriteByte('\n')
			}
			text.WriteString(renderTOMLAOT(entry.key, entry.item.aot, aotPrefix))
		default:
			text.WriteString(renderTOMLSimple(entry.key, &entry.item, simplePrefix))
		}
	}
}

func noTOMLPrefix(*tomlEntry) *string { return nil }

func renderTOMLTableBody(table *tomlTable) string {
	var text strings.Builder
	renderTOMLEntries(&text, table.body.body, noTOMLPrefix, nil, nil)
	return text.String()
}

func renderTOMLRoot(root *tomlContainer) string {
	var text strings.Builder
	renderTOMLEntries(&text, root.body, noTOMLPrefix, nil, nil)
	return text.String()
}

func renderTOMLTable(key *tomlKey, table *tomlTable, prefix *string) string {
	dotted := key != nil && key.dotted
	keyText := table.displayName
	if keyText == "" && key != nil {
		keyText = prefixedTOMLName(prefix, key.asString())
	}
	directValues, nestedDotted := false, false
	for index := range table.body.body {
		entry := &table.body.body[index]
		switch entry.item.kind {
		case tomlTableItem:
			nestedDotted = nestedDotted || !dotted && entry.key != nil && entry.key.dotted
		case tomlAOTItem, tomlWhitespaceItem, tomlNullItem:
		default:
			directValues = true
		}
	}
	var text strings.Builder
	if !table.implicit || directValues && !dotted || nestedDotted {
		open, closing := "[", "]"
		if table.aotElement {
			open, closing = "[[", "]]"
		}
		text.WriteString(table.trivia.indent + open + keyText + closing + table.trivia.commentWS + table.trivia.comment + table.trivia.trail)
		if !strings.Contains(table.trivia.trail, "\n") && len(table.body.body) > 0 {
			text.WriteByte('\n')
		}
	} else if table.trivia.indent == "\n" {
		text.WriteString(table.trivia.indent)
	}
	childPrefix := func(entry *tomlEntry) *string {
		if entry.item.table.implicit && entry.key != nil && entry.key.dotted && !dotted {
			return nil
		}
		return &keyText
	}
	var simplePrefix *string
	if dotted {
		simplePrefix = &keyText
	}
	renderTOMLEntries(&text, table.body.body, childPrefix, &keyText, simplePrefix)
	return text.String()
}

func renderTOMLAOT(key *tomlKey, members []tomlTable, prefix *string) string {
	name := ""
	if key != nil {
		name = prefixedTOMLName(prefix, key.asString())
	}
	var text strings.Builder
	for index := range members {
		text.WriteString(renderTOMLAOTTable(&members[index], name))
	}
	return text.String()
}

func renderTOMLAOTTable(table *tomlTable, keyText string) string {
	var text strings.Builder
	text.WriteString(table.trivia.indent + "[[" + keyText + "]]" + table.trivia.commentWS + table.trivia.comment + table.trivia.trail)
	for index := range table.body.body {
		entry := &table.body.body[index]
		switch entry.item.kind {
		case tomlTableItem:
			prefix := &keyText
			if entry.item.table.implicit && entry.key != nil && entry.key.dotted {
				prefix = nil
			}
			text.WriteString(renderTOMLTable(entry.key, &entry.item.table, prefix))
		case tomlAOTItem:
			text.WriteString(renderTOMLAOT(entry.key, entry.item.aot, &keyText))
		default:
			text.WriteString(renderTOMLSimple(entry.key, &entry.item, nil))
		}
	}
	return text.String()
}
