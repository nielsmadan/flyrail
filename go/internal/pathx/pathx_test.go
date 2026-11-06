package pathx

import (
	"testing"

	"github.com/google/go-cmp/cmp"
)

type parentCase struct {
	path, want string
	ok         bool
}

func TestParentKeepsTheRemainingTextAsRustDoes(t *testing.T) {
	for _, c := range []parentCase{
		{"/", "", false},
		{"", "", false},
		{".", "", true},
		{"a", "", true},
		{"./a", ".", true},
		{"/a", "/", true},
		{"/a//b/", "/a", true},
		{"/a/./b", "/a", true},
		{"a//./b/c", "a//./b", true},
		{"//a", "/", true},
		{"/a/../b", "/a/..", true},
	} {
		if got, ok := Parent(c.path); got != c.want || ok != c.ok {
			t.Errorf("Parent(%q) = %q, %v; want %q, %v", c.path, got, ok, c.want, c.ok)
		}
	}
}

func TestNormalizedParentRebuildsTheComponents(t *testing.T) {
	for _, c := range []parentCase{
		{"/", "", false},
		{"", "", false},
		{"a", "", true},
		{"./a", ".", true},
		{"/a", "/", true},
		{"/a//b/", "/a", true},
		{"a//./b/c", "a/b", true},
	} {
		if got, ok := NormalizedParent(c.path); got != c.want || ok != c.ok {
			t.Errorf("NormalizedParent(%q) = %q, %v; want %q, %v", c.path, got, ok, c.want, c.ok)
		}
	}
}

func TestDirectoryNamesTheContainingDirectory(t *testing.T) {
	for path, want := range map[string]string{
		"x.zip": ".", "/x.zip": "/", "a/b.zip": "a", "a//b.zip": "a", "x.zip/": ".", "/": "/", "": "",
	} {
		if got := Directory(path); got != want {
			t.Errorf("Directory(%q) = %q, want %q", path, got, want)
		}
	}
}

func TestComponentsJoinAndFileNameFollowRustPath(t *testing.T) {
	if diff := cmp.Diff([]string{"/", "a", "b", "..", "c"}, Components("//a/./b//../c/")); diff != "" {
		t.Errorf("Components (-want +got):\n%s", diff)
	}
	if diff := cmp.Diff([]string{".", "a"}, Components("./a")); diff != "" {
		t.Errorf("Components (-want +got):\n%s", diff)
	}
	if got := Join("/a/", "b", "/c", "d"); got != "/c/d" {
		t.Errorf("Join = %q", got)
	}
	if got := Join("", "a", "b"); got != "a/b" {
		t.Errorf("Join from empty = %q", got)
	}
	names := []struct {
		path, want string
		ok         bool
	}{
		{"/", "", false},
		{"/a/..", "", false},
		{"/a/b/", "b", true},
		{"/a/./b", "b", true},
	}
	for _, c := range names {
		if got, ok := FileName(c.path); got != c.want || ok != c.ok {
			t.Errorf("FileName(%q) = %q, %v; want %q, %v", c.path, got, ok, c.want, c.ok)
		}
	}
	if !HasPrefix("/home/u", "/home") || HasPrefix("/homes", "/home") || !HasPrefix("/x", "/") {
		t.Error("HasPrefix does not compare whole components")
	}
}

func TestAbsoluteFollowsRustStdPathAbsolute(t *testing.T) {
	for path, want := range map[string]string{
		"/a/./b//c": "/a/b/c",
		"//a/b":     "//a/b",
		"///a":      "/a",
		"a/../b/":   "/work/a/../b/",
		"./a":       "/work/a",
		".":         "/work",
		"/":         "/",
	} {
		if got := Absolute(path, "/work"); got != want {
			t.Errorf("Absolute(%q) = %q, want %q", path, got, want)
		}
	}
}
