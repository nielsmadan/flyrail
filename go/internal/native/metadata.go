package native

import (
	"bytes"
	"encoding/hex"
	"slices"
	"strconv"
)

type attribute struct {
	name  []byte
	value []byte
}

func ancestorMetadata(flags uint32, attributes []attribute, acl []byte) []byte {
	sorted := slices.Clone(attributes)
	slices.SortFunc(sorted, func(a, b attribute) int {
		if order := bytes.Compare(a.name, b.name); order != 0 {
			return order
		}
		return bytes.Compare(a.value, b.value)
	})
	out := []byte(`{"acl":"`)
	out = hex.AppendEncode(out, acl)
	out = append(out, `","flags":`...)
	out = strconv.AppendUint(out, uint64(flags), 10)
	out = append(out, `,"schema_version":1,"xattrs":[`...)
	for index, item := range sorted {
		if index > 0 {
			out = append(out, ',')
		}
		out = append(out, `["`...)
		out = hex.AppendEncode(out, item.name)
		out = append(out, `","`...)
		out = hex.AppendEncode(out, item.value)
		out = append(out, `"]`...)
	}
	return append(out, "]}"...)
}
