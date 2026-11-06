package hookruntime

import (
	"bytes"
	"os"
	"path/filepath"
	"slices"
	"testing"
)

func TestFilesMatchThePythonRuntime(t *testing.T) {
	files := Files()
	names := make([]string, 0, len(files))
	for _, file := range files {
		names = append(names, file.Name)
		reference, err := os.ReadFile(filepath.Join("..", "..", "..", "python", "src", "flyrail", "runtime", file.Name))
		if os.IsNotExist(err) && os.Getenv("FLYRAIL_REQUIRE_FIXTURES") != "1" {
			t.Skip("python/src/flyrail/runtime is not available")
		}
		if err != nil {
			t.Fatal(err)
		}
		if !bytes.Equal(file.Data, reference) {
			t.Errorf("%s differs from python/src/flyrail/runtime/%s", file.Name, file.Name)
		}
	}
	if want := []string{"runner.mts", "bridge.mts", "process.mts"}; !slices.Equal(names, want) {
		t.Fatalf("names = %v, want %v", names, want)
	}
}

func TestFilesReturnsIndependentCopies(t *testing.T) {
	first := Files()
	first[0].Data[0] ^= 0xff
	if bytes.Equal(first[0].Data, Files()[0].Data) {
		t.Fatal("mutating a returned module changed the embedded bytes")
	}
}
