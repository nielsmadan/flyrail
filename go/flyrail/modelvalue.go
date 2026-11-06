package flyrail

import (
	"encoding/hex"
	"slices"
	"strings"
)

func field(key string, value Value) ObjectMember {
	return ObjectMember{Key: key, Value: value}
}

func modelText(text string) Value {
	return String{text: text}
}

func valueDepth(value Value) int {
	depth, _ := checkValue(value)
	return depth
}

func modelArray(items []Value) Value {
	depth := 0
	for _, item := range items {
		depth = max(depth, valueDepth(item))
	}
	return Array{items: items, depth: depth + 1}
}

func modelList[T any](items []T, model func(T) Value) Value {
	values := make([]Value, len(items))
	for index, item := range items {
		values[index] = model(item)
	}
	return modelArray(values)
}

func modelObject(members ...ObjectMember) Value {
	sorted := slices.Clone(members)
	slices.SortFunc(sorted, func(a, b ObjectMember) int { return strings.Compare(a.Key, b.Key) })
	depth := 0
	for _, member := range sorted {
		depth = max(depth, valueDepth(member.Value))
	}
	return Object{members: sorted, depth: depth + 1}
}

func typedRecord(typeName string, fields ...ObjectMember) Value {
	return modelObject(field("fields", modelObject(fields...)), field("type", modelText(typeName)))
}

func modelBytes(data []byte) Value {
	return modelObject(field("bytes", modelText(hex.EncodeToString(data))))
}

func modelEntry(entry BundleEntry) Value {
	data := Value(Null{})
	if !entry.Dir {
		data = modelBytes(entry.Data)
	}
	return typedRecord("BundleEntry",
		field("data", data),
		field("executable", Bool(entry.Executable)),
		field("path", modelText(entry.Path)),
	)
}

func modelOfValue(value Value) Value {
	switch v := value.(type) {
	case Array:
		return typedRecord("ArrayValue", field("items", modelList(v.items, modelOfValue)))
	case Object:
		return typedRecord("ObjectValue", field("items", modelList(v.members, func(member ObjectMember) Value {
			return modelArray([]Value{modelText(member.Key), modelOfValue(member.Value)})
		})))
	}
	return typedRecord("Scalar", field("value", value))
}

func modelEnum[T ~string](value T) Value {
	if value == "" {
		return Null{}
	}
	return modelText(string(value))
}
