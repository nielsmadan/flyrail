package native

import (
	"encoding/hex"
	"testing"
)

func TestAncestorMetadataPinsExactBytes(t *testing.T) {
	cases := []struct {
		name       string
		flags      uint32
		attributes []attribute
		acl        string
		want       string
	}{
		{
			name: "empty",
			want: `{"acl":"","flags":0,"schema_version":1,"xattrs":[]}`,
		},
		{
			name:       "one attribute",
			attributes: []attribute{{name: []byte("flyrail.test"), value: []byte("value")}},
			want:       `{"acl":"","flags":0,"schema_version":1,"xattrs":[["666c797261696c2e74657374","76616c7565"]]}`,
		},
		{
			name:  "sorted attributes, flags and ACL",
			flags: 4,
			attributes: []attribute{
				{name: []byte("zeta"), value: []byte("2")},
				{name: []byte("alpha"), value: []byte("1")},
			},
			acl: "user:joe:allow:read\n",
			want: `{"acl":"` + hex.EncodeToString([]byte("user:joe:allow:read\n")) + `","flags":4,"schema_version":1,"xattrs":[["` +
				hex.EncodeToString([]byte("alpha")) + `","31"],["` + hex.EncodeToString([]byte("zeta")) + `","32"]]}`,
		},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			if got := string(ancestorMetadata(c.flags, c.attributes, []byte(c.acl))); got != c.want {
				t.Fatalf("ancestorMetadata = %s, want %s", got, c.want)
			}
		})
	}
}

func TestAncestorMetadataLeavesTheCallersOrder(t *testing.T) {
	attributes := []attribute{{name: []byte("b")}, {name: []byte("a")}}
	ancestorMetadata(0, attributes, nil)
	if string(attributes[0].name) != "b" || string(attributes[1].name) != "a" {
		t.Fatalf("attributes reordered to %q, %q", attributes[0].name, attributes[1].name)
	}
}
