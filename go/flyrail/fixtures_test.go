package flyrail

import (
	"os"
	"path/filepath"
	"testing"
)

func readFixture(name string) ([]byte, error) {
	return os.ReadFile(filepath.Join("..", "..", "spec", "fixtures", name))
}

func fixture(t testing.TB, name string) []byte {
	t.Helper()
	data, err := readFixture(name)
	if os.IsNotExist(err) {
		if os.Getenv("FLYRAIL_REQUIRE_FIXTURES") == "1" {
			t.Fatalf("fixture %s is required but missing", name)
		}
		t.Skipf("skipping: spec fixture %s is not available", name)
	}
	if err != nil {
		t.Fatal(err)
	}
	return data
}
