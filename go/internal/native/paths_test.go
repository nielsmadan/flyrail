package native

import "testing"

func TestManagementNameUsesThePortableKey(t *testing.T) {
	for _, name := range []string{".flyrail-abc.txn", ".FLYRAIL-deadbeef.TXN"} {
		if !ManagementName(name) {
			t.Errorf("ManagementName(%q) = false", name)
		}
	}
	for _, name := range []string{"flyrail-abc.txn", ".flyrail-abc.tx"} {
		if ManagementName(name) {
			t.Errorf("ManagementName(%q) = true", name)
		}
	}
}
