//go:build darwin || linux

package flyrail

import (
	"errors"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/google/go-cmp/cmp"
	"golang.org/x/sys/unix"

	"github.com/nielsmadan/flyrail/go/internal/zipx"
	"github.com/nielsmadan/flyrail/go/internal/zipx/ziptest"
)

const archiveManifest = `{"schema_version":1,"id":"tiny","version":"1","skills":[{"name":"a","path":"a"}]}`

func openSource(t *testing.T, root string) *directorySource {
	t.Helper()
	source, err := newDirectorySource(t.Context(), root)
	if err != nil {
		t.Fatalf("newDirectorySource: %v", err)
	}
	return source
}

func entryPaths(entries []BundleEntry) []string {
	paths := make([]string, len(entries))
	for index, entry := range entries {
		paths[index] = entry.Path
	}
	return paths
}

func TestDirectorySourceReadsTheManifestAndWalksSkillTrees(t *testing.T) {
	root := tempRoot(t)
	skill := writeBundle(t, root, defaultManifest)
	mkdir(t, filepath.Join(skill, "empty"))
	writeFile(t, filepath.Join(skill, "tools", "run.sh"), "#!/bin/sh\n")
	source := openSource(t, root)
	if manifest := must(source.readManifest()); string(manifest) != defaultManifest {
		t.Fatalf("manifest = %q", manifest)
	}
	if data := must(source.readFile("skills/review/tools/run.sh")); string(data) != "#!/bin/sh\n" {
		t.Fatalf("file = %q", data)
	}
	entries := must(source.treeEntries("skills/review", "review", []string{"tools/run.sh"}))
	sortEntries(entries)
	executable := file("review/tools/run.sh", "#!/bin/sh\n")
	executable.Executable = true
	want := []BundleEntry{dir("review"), file("review/SKILL.md", defaultSkill), dir("review/empty"), dir("review/tools"), executable}
	if diff := cmp.Diff(want, entries); diff != "" {
		t.Fatalf("entries (-want +got):\n%s", diff)
	}
	if err := source.finish(); err != nil {
		t.Fatal(err)
	}
}

func TestDirectorySourceRejectsMissingCaseMismatchedAndNonfilePaths(t *testing.T) {
	root := tempRoot(t)
	writeBundle(t, root, defaultManifest)
	if err := os.Rename(filepath.Join(root, "skills"), filepath.Join(root, "SKILLS")); err != nil {
		t.Fatal(err)
	}
	source := openSource(t, root)
	if got := invalidMessage(t, errorOf(source.readFile("skills/review/SKILL.md"))); got != "missing, case-mismatched or ambiguous source path: skills/review/SKILL.md" {
		t.Errorf("mismatch: %q", got)
	}
	if got := invalidMessage(t, errorOf(source.readFile("SKILLS"))); got != "expected a source file: "+filepath.Join(root, "SKILLS") {
		t.Errorf("directory: %q", got)
	}
	if got := invalidMessage(t, errorOf(source.treeEntries("SKILLS/review/SKILL.md", "review", nil))); got != "skill source must be a directory: SKILLS/review/SKILL.md" {
		t.Errorf("file tree: %q", got)
	}
}

func TestDirectorySourceRejectsSymlinkedSources(t *testing.T) {
	root := tempRoot(t)
	skill := writeBundle(t, root, defaultManifest)
	external := tempRoot(t)
	writeFile(t, filepath.Join(external, "SKILL.md"), "external")
	linked := filepath.Join(skill, "linked")
	if err := os.Symlink(external, linked); err != nil {
		t.Fatal(err)
	}
	want := "source symlinks and special files are not supported: " + linked
	if got := invalidMessage(t, errorOf(openSource(t, root).treeEntries("skills/review", "review", nil))); got != want {
		t.Errorf("linked child: %q", got)
	}
	if got := invalidMessage(t, errorOf(OpenBundle(t.Context(), linked))); got != want {
		t.Errorf("linked root: %q", got)
	}
}

func TestDirectorySourceRejectsFIFOsWithoutBlocking(t *testing.T) {
	root := tempRoot(t)
	skill := writeBundle(t, root, defaultManifest)
	pipe := filepath.Join(skill, "pipe")
	if err := unix.Mkfifo(pipe, 0o600); err != nil {
		t.Fatal(err)
	}
	done := make(chan error, 1)
	go func() { done <- errorOf(OpenBundle(t.Context(), root)) }()
	select {
	case err := <-done:
		if got := invalidMessage(t, err); got != "source symlinks and special files are not supported: "+pipe {
			t.Fatalf("fifo: %q", got)
		}
	case <-time.After(10 * time.Second):
		t.Fatal("loading a bundle with a FIFO blocked")
	}
}

func TestDirectorySourceDetectsAFileSwappedForAFIFO(t *testing.T) {
	root := tempRoot(t)
	skill := writeBundle(t, root, defaultManifest)
	source := openSource(t, root)
	markdown := filepath.Join(skill, "SKILL.md")
	before := must(source.observe(markdown))
	if err := os.Remove(markdown); err != nil {
		t.Fatal(err)
	}
	if err := unix.Mkfifo(markdown, 0o600); err != nil {
		t.Fatal(err)
	}
	done := make(chan error, 1)
	go func() { done <- errorOf(source.snapshotFile(markdown, before, -1)) }()
	select {
	case err := <-done:
		if got := invalidMessage(t, err); got != "bundle source changed while opening: "+markdown {
			t.Fatalf("swap: %q", got)
		}
	case <-time.After(10 * time.Second):
		t.Fatal("reading a swapped FIFO blocked")
	}
}

func TestDirectorySourceSnapshotsHardLinkedFilesAsContent(t *testing.T) {
	root := tempRoot(t)
	skill := writeBundle(t, root, defaultManifest)
	if err := os.Link(filepath.Join(skill, "SKILL.md"), filepath.Join(skill, "copy.md")); err != nil {
		t.Fatal(err)
	}
	want := []BundleEntry{dir("review"), file("review/SKILL.md", defaultSkill), file("review/copy.md", defaultSkill)}
	if diff := cmp.Diff(want, openDir(t, root).Entries()); diff != "" {
		t.Fatalf("entries (-want +got):\n%s", diff)
	}
}

func preserved(t *testing.T, directory, name string) bool {
	t.Helper()
	listing, err := os.ReadDir(directory)
	if err != nil {
		t.Fatal(err)
	}
	return slices.ContainsFunc(listing, func(entry os.DirEntry) bool { return entry.Name() == name })
}

func TestDirectorySourceRejectsNonportableActualNames(t *testing.T) {
	root := tempRoot(t)
	skill := writeBundle(t, root, defaultManifest)
	checked := 0
	for _, name := range []string{"CON.txt", "trailing.", "bad\x7f", "bad\u0085", "bad?", "cafe\u0301"} {
		selected := filepath.Join(skill, name)
		if os.Mkdir(selected, 0o700) != nil || !preserved(t, skill, name) {
			_ = os.Remove(selected)
			continue
		}
		got := invalidMessage(t, errorOf(openSource(t, root).treeEntries("skills/review", "review", nil)))
		if !strings.HasPrefix(got, "source name ") {
			t.Errorf("%q: %q", name, got)
		}
		if err := os.Remove(selected); err != nil {
			t.Fatal(err)
		}
		checked++
	}
	if checked == 0 {
		t.Fatal("host rejected every nonportable name")
	}
}

func TestDirectorySourceRejectsNamesThatAreNotUTF8(t *testing.T) {
	root := tempRoot(t)
	skill := writeBundle(t, root, defaultManifest)
	if err := os.Mkdir(filepath.Join(skill, "raw\xff"), 0o700); err != nil {
		t.Skipf("host cannot create non-UTF-8 names: %v", err)
	}
	got := invalidMessage(t, errorOf(openSource(t, root).treeEntries("skills/review", "review", nil)))
	if got != "source name must contain NFC-normalized Unicode scalar values" {
		t.Fatalf("got %q", got)
	}
}

func caseSensitive(t *testing.T, root string) bool {
	t.Helper()
	probe := filepath.Join(root, "case-probe")
	mkdir(t, probe)
	defer func() { _ = os.Remove(probe) }()
	shouted := filepath.Join(root, "CASE-PROBE")
	if os.Mkdir(shouted, 0o700) != nil {
		return false
	}
	_ = os.Remove(shouted)
	return true
}

func TestDirectorySourceRejectsPortableNameCollisions(t *testing.T) {
	root := tempRoot(t)
	if !caseSensitive(t, root) {
		t.Skip("case-insensitive file system cannot hold colliding names")
	}
	skill := writeBundle(t, root, defaultManifest)
	mkdir(t, filepath.Join(skill, "same"))
	mkdir(t, filepath.Join(skill, "SAME"))
	got := invalidMessage(t, errorOf(openSource(t, root).treeEntries("skills/review", "review", nil)))
	if !strings.HasPrefix(got, "portable source name collision: ") {
		t.Fatalf("got %q", got)
	}
}

func TestDirectorySourceDetectsSourcesThatChangeAfterBeingRead(t *testing.T) {
	root := tempRoot(t)
	skill := writeBundle(t, root, defaultManifest)
	source := openSource(t, root)
	must(source.readManifest())
	must(source.treeEntries("skills/review", "review", nil))
	writeFile(t, filepath.Join(skill, "SKILL.md"), "concurrent edit")
	if got := invalidMessage(t, source.finish()); got != "bundle source changed while loading: "+filepath.Join(skill, "SKILL.md") {
		t.Fatalf("got %q", got)
	}
}

func TestDirectorySourceDetectsListingChanges(t *testing.T) {
	root := tempRoot(t)
	skill := writeBundle(t, root, defaultManifest)
	source := openSource(t, root)
	must(source.treeEntries("skills/review", "review", nil))
	added := filepath.Join(skill, "added")
	writeFile(t, added, "late")
	err := source.finish()
	if got := invalidMessage(t, err); !strings.HasPrefix(got, "bundle source changed while loading: ") {
		t.Fatalf("got %q", got)
	}
}

func zipSourceOf(t *testing.T, resource string, specs ...ziptest.Spec) (*zipSource, error) {
	t.Helper()
	archive, err := zipx.Open(ziptest.Write(specs...), zipx.DefaultLimits())
	if err != nil {
		t.Fatalf("test archive must parse: %v", err)
	}
	return newZipSource(archive, resource, "package.zip")
}

func archiveMembers() []ziptest.Spec {
	return []ziptest.Spec{
		ziptest.File("flyrail/flyrail.json", []byte(archiveManifest)),
		ziptest.File("flyrail/a/SKILL.md", nil),
	}
}

func withExtra(names ...string) []ziptest.Spec {
	specs := archiveMembers()
	for _, name := range names {
		specs = append(specs, ziptest.File("flyrail/"+name, []byte("bad")))
	}
	return specs
}

func TestZipSourceInfersDirectoriesAndReadsMembers(t *testing.T) {
	source := must(zipSourceOf(t, "flyrail", archiveMembers()...))
	if manifest := must(source.readManifest()); string(manifest) != archiveManifest {
		t.Fatalf("manifest = %q", manifest)
	}
	if data := must(source.readFile("a/SKILL.md")); len(data) != 0 {
		t.Fatalf("file = %q", data)
	}
	if diff := cmp.Diff([]BundleEntry{dir("a"), file("a/SKILL.md", "")}, must(source.treeEntries("a", "a", nil))); diff != "" {
		t.Fatalf("entries (-want +got):\n%s", diff)
	}
	if got := invalidMessage(t, errorOf(source.readFile("absent"))); got != "expected a source file: absent" {
		t.Fatalf("absent: %q", got)
	}
}

func TestZipSourceRejectsNonportableAndTraversalNames(t *testing.T) {
	for _, name := range []string{
		"a/CON.txt", "a/trailing.", "a/trailing ", "a/bad\x7f", "a/bad\u0085", "a/bad?", "a/cafe\u0301",
		"a/../outside", "a//double", "a/sub/../../outside", "a/C:/escape", "a/back\\slash", "a/double//slash",
	} {
		if got := invalidMessage(t, errorOf(zipSourceOf(t, "flyrail", withExtra(name)...))); !strings.HasPrefix(got, "archive path ") {
			t.Errorf("%q: %q", name, got)
		}
	}
}

func TestZipSourceRejectsCollisionsAndAmbiguousDirectories(t *testing.T) {
	for _, names := range [][]string{
		{"a/same/", "a/SAME/"},
		{"a/straße", "a/strasse"},
		{"a/parent/a", "a/PARENT/b"},
		{"a/kept", "a/kept/child"},
		{"a/kept/child", "a/kept"},
		{"a/extra//"},
	} {
		got := invalidMessage(t, errorOf(zipSourceOf(t, "flyrail", withExtra(names...)...)))
		if !strings.Contains(got, "collision") && !strings.Contains(got, "ambiguous") {
			t.Errorf("%q: %q", names, got)
		}
	}
}

func TestZipSourceRejectsDuplicateMembers(t *testing.T) {
	specs := append(archiveMembers(), ziptest.File("flyrail/a/SKILL.md", nil))
	if got := invalidMessage(t, errorOf(zipSourceOf(t, "flyrail", specs...))); got != "duplicate or ambiguous archive entry: flyrail/a/SKILL.md" {
		t.Errorf("file: %q", got)
	}
	specs = append(archiveMembers(), ziptest.File("flyrail/a/", nil), ziptest.File("flyrail/a/", nil))
	if got := invalidMessage(t, errorOf(zipSourceOf(t, "flyrail", specs...))); got != "duplicate or ambiguous archive entry: flyrail/a/" {
		t.Errorf("directory: %q", got)
	}
}

func TestZipSourceRejectsSymlinksSpecialFilesAndReparsePoints(t *testing.T) {
	for _, kind := range []uint32{0o120000, 0o010000, 0o140000, 0o020000, 0o040000} {
		special := ziptest.Blank()
		special.Name = "flyrail/a/special"
		special.Data = []byte("target")
		special.ExternalAttr = (kind | 0o600) << 16
		got := invalidMessage(t, errorOf(zipSourceOf(t, "flyrail", append(archiveMembers(), special)...)))
		if got != "archive symlinks and special files are not supported: flyrail/a/special" {
			t.Errorf("kind %o: %q", kind, got)
		}
	}
	reparse := ziptest.Blank()
	reparse.Name = "flyrail/a/reparse"
	reparse.Data = []byte("target")
	reparse.CreateSystem = 0
	reparse.ExternalAttr = 0x400
	got := invalidMessage(t, errorOf(zipSourceOf(t, "flyrail", append(archiveMembers(), reparse)...)))
	if got != "archive reparse points are not supported: flyrail/a/reparse" {
		t.Errorf("reparse: %q", got)
	}
}

func TestZipSourceRejectsNULTruncatedNames(t *testing.T) {
	hidden := ziptest.Blank()
	hidden.RawName = []byte("flyrail/a/name\x00hidden")
	hidden.Data = []byte("data")
	got := invalidMessage(t, errorOf(zipSourceOf(t, "flyrail", append(archiveMembers(), hidden)...)))
	if got != `ambiguous archive entry: 'flyrail/a/name\x00hidden'` {
		t.Fatalf("got %q", got)
	}
}

func TestZipSourceRejectsAmbiguousResourceRootsAndCollidingAncestors(t *testing.T) {
	shouted := append(archiveMembers(), ziptest.File("FLYRAIL/", nil))
	if got := invalidMessage(t, errorOf(zipSourceOf(t, "flyrail", shouted...))); got != "portable archive name collision: FLYRAIL" {
		t.Errorf("shouted: %q", got)
	}
	var nested []ziptest.Spec
	for _, spec := range archiveMembers() {
		spec.Name = strings.Replace(spec.Name, "flyrail/", "resources/flyrail/", 1)
		nested = append(nested, spec)
	}
	for _, unrelatedFirst := range []bool{false, true} {
		member := ziptest.File("RESOURCES/unrelated/file", nil)
		specs := append(slices.Clone(nested), member)
		reported := "RESOURCES"
		if unrelatedFirst {
			specs = append([]ziptest.Spec{member}, nested...)
			reported = "resources"
		}
		if got := invalidMessage(t, errorOf(zipSourceOf(t, "resources/flyrail", specs...))); got != "portable archive name collision: "+reported {
			t.Errorf("unrelated first %v: %q", unrelatedFirst, got)
		}
	}
}

func TestZipSourceAllowsUnrelatedNonportableMembers(t *testing.T) {
	specs := append(archiveMembers(),
		ziptest.File("unrelated/CON.txt", nil), ziptest.File("UNRELATED/file", nil), ziptest.File("outside/CON.txt", nil))
	source := must(zipSourceOf(t, "flyrail", specs...))
	if diff := cmp.Diff([]BundleEntry{dir("a"), file("a/SKILL.md", "")}, must(source.treeEntries("a", "a", nil))); diff != "" {
		t.Fatalf("entries (-want +got):\n%s", diff)
	}
}

func TestZipSourceIgnoresEncryptedUnrelatedMembersUntilRead(t *testing.T) {
	secret := ziptest.File("flyrail/unrelated/secret.bin", []byte("secret"))
	secret.FlagBits = 1
	source := must(zipSourceOf(t, "flyrail", append(archiveMembers(), secret)...))
	if diff := cmp.Diff([]BundleEntry{dir("a"), file("a/SKILL.md", "")}, must(source.treeEntries("a", "a", nil))); diff != "" {
		t.Fatalf("entries (-want +got):\n%s", diff)
	}
	_, err := source.readFile("unrelated/secret.bin")
	failure, ok := errors.AsType[*BundleError](err)
	if !ok || failure.Message != "archive member is encrypted" {
		t.Fatalf("err = %v", err)
	}
	if _, invalid := errors.AsType[*ValueError](err); invalid {
		t.Fatal("an unreadable member was reported as invalid input")
	}
}

func TestZipSourceRequiresAnExistingResourceDirectoryWithAManifest(t *testing.T) {
	for _, resource := range []string{"missing", "flyrail/flyrail.json"} {
		if got := invalidMessage(t, errorOf(zipSourceOf(t, resource, archiveMembers()...))); got != "package resource must be an existing directory" {
			t.Errorf("%s: %q", resource, got)
		}
	}
	source := must(zipSourceOf(t, "flyrail", archiveMembers()[1:]...))
	if got := invalidMessage(t, errorOf(source.readManifest())); got != "package resource must contain a flyrail.json file" {
		t.Errorf("manifest: %q", got)
	}
	if got := invalidMessage(t, errorOf(source.treeEntries("absent", "absent", nil))); got != "skill source must be an existing directory: absent" {
		t.Errorf("tree: %q", got)
	}
}

func TestMemorySourceReadsFilesAndRebasesSkillTrees(t *testing.T) {
	source := must(newMemorySource([]byte("{}"), []BundleEntry{
		file("skills/review/SKILL.md", "markdown"),
		file("skills/review/tools/run.sh", "#!/bin/sh"),
	}))
	if data := must(source.readFile("skills/review/SKILL.md")); string(data) != "markdown" {
		t.Fatalf("file = %q", data)
	}
	for _, path := range []string{"skills/review", "absent"} {
		if got := invalidMessage(t, errorOf(source.readFile(path))); got != "expected a source file: "+path {
			t.Errorf("%s: %q", path, got)
		}
	}
	entries := must(source.treeEntries("skills/review", "review", []string{"tools/run.sh"}))
	if diff := cmp.Diff([]string{"review", "review/SKILL.md", "review/tools", "review/tools/run.sh"}, entryPaths(entries)); diff != "" {
		t.Fatalf("paths (-want +got):\n%s", diff)
	}
	if !entries[3].Executable {
		t.Fatal("declared executable lost")
	}
	if got := invalidMessage(t, errorOf(source.treeEntries("skills/review/SKILL.md", "review", nil))); got != "skill source must be an existing directory: skills/review/SKILL.md" {
		t.Fatalf("file tree: %q", got)
	}
}
