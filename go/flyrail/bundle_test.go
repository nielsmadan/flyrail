package flyrail

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"io/fs"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"testing/fstest"

	"github.com/google/go-cmp/cmp"

	"github.com/nielsmadan/flyrail/go/internal/jsonx"
	"github.com/nielsmadan/flyrail/go/internal/zipx/ziptest"
)

const defaultManifest = `{"schema_version":1,"id":"team","version":"1","skills":[{"name":"review","path":"skills/review"}]}`

const defaultSkill = "---\r\ninvalid yaml: [\r\n---\r\n\xff\x00"

func tempRoot(t *testing.T) string {
	t.Helper()
	root, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	return root
}

func writeFile(t *testing.T, path, data string) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(data), 0o600); err != nil {
		t.Fatal(err)
	}
}

func mkdir(t *testing.T, path string) {
	t.Helper()
	if err := os.MkdirAll(path, 0o700); err != nil {
		t.Fatal(err)
	}
}

func writeBundle(t *testing.T, root, manifest string) string {
	t.Helper()
	writeFile(t, filepath.Join(root, "flyrail.json"), manifest)
	skill := filepath.Join(root, "skills", "review")
	writeFile(t, filepath.Join(skill, "SKILL.md"), defaultSkill)
	return skill
}

func invalidMessage(t *testing.T, err error) string {
	t.Helper()
	if err == nil {
		t.Fatal("expected an error")
	}
	failure, ok := errors.AsType[*BundleError](err)
	if !ok {
		t.Fatalf("error %T %v is not a *BundleError", err, err)
	}
	if _, ok := errors.AsType[*ValueError](failure.Err); !ok {
		t.Fatalf("error %q is not invalid input: %T", failure.Message, failure.Err)
	}
	return failure.Message
}

func file(path, data string) BundleEntry {
	return BundleEntry{Path: path, Data: []byte(data)}
}

func dir(path string) BundleEntry {
	return BundleEntry{Path: path, Dir: true}
}

func openDir(t *testing.T, root string) *Bundle {
	t.Helper()
	bundle, err := OpenBundle(t.Context(), root)
	if err != nil {
		t.Fatalf("OpenBundle(%s): %v", root, err)
	}
	return bundle
}

type bundleCase struct {
	name        string
	manifest    string
	directories []string
	files       []BundleEntry
	entries     []BundleEntry
	digest      string
	framing     []byte
}

func fromHex(t *testing.T, text string) []byte {
	t.Helper()
	data, err := hex.DecodeString(text)
	if err != nil {
		t.Fatal(err)
	}
	return data
}

func fixtureText(t *testing.T, value jsonx.Value, key string) string {
	t.Helper()
	field, _ := value.Get(key)
	text, ok := field.AsString()
	if !ok {
		t.Fatalf("fixture field %s must be a string", key)
	}
	return text
}

func fixtureItems(t *testing.T, value jsonx.Value, key string) []jsonx.Value {
	t.Helper()
	field, _ := value.Get(key)
	items, ok := field.Items()
	if !ok {
		t.Fatalf("fixture field %s must be an array", key)
	}
	return items
}

func bundleCases(t *testing.T) []bundleCase {
	t.Helper()
	raw, err := jsonx.Parse(fixture(t, "bundles.json"))
	if err != nil {
		t.Fatal(err)
	}
	var cases []bundleCase
	for _, item := range fixtureItems(t, raw, "cases") {
		manifest, _ := item.Get("manifest")
		if version, _ := manifest.Get("schema_version"); !jsonx.Equal(version, jsonx.Int(1)) {
			continue
		}
		current := bundleCase{
			name:     fixtureText(t, item, "name"),
			manifest: string(jsonx.Encode(manifest)),
			digest:   fixtureText(t, item, "content_digest"),
		}
		for _, directory := range fixtureItems(t, item, "directories") {
			path, _ := directory.AsString()
			current.directories = append(current.directories, path)
		}
		for _, entry := range fixtureItems(t, item, "files") {
			current.files = append(current.files, BundleEntry{
				Path: fixtureText(t, entry, "path"),
				Data: fromHex(t, fixtureText(t, entry, "data_hex")),
			})
		}
		for _, entry := range fixtureItems(t, item, "entries") {
			expected := BundleEntry{Path: fixtureText(t, entry, "path"), Dir: true}
			if data, _ := entry.Get("data_hex"); !data.IsNull() {
				expected.Data, expected.Dir = fromHex(t, fixtureText(t, entry, "data_hex")), false
			}
			executable, _ := entry.Get("executable")
			expected.Executable, _ = executable.AsBool()
			current.entries = append(current.entries, expected)
		}
		if framing, ok := item.Get("framing_hex"); ok {
			text, _ := framing.AsString()
			current.framing = fromHex(t, text)
		}
		cases = append(cases, current)
	}
	return cases
}

func caseArchive(c bundleCase, resource string) []byte {
	specs := []ziptest.Spec{ziptest.File(resource+"/flyrail.json", []byte(c.manifest))}
	for _, directory := range c.directories {
		specs = append(specs, ziptest.Directory(resource+"/"+directory))
	}
	for _, entry := range c.files {
		specs = append(specs, ziptest.File(resource+"/"+entry.Path, entry.Data))
	}
	return ziptest.Write(specs...)
}

func TestMatchesTheSharedSchemaOneBundleFixtures(t *testing.T) {
	cases := bundleCases(t)
	if len(cases) == 0 {
		t.Fatal("no schema-1 bundle fixtures")
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			parent := tempRoot(t)
			root := filepath.Join(parent, "flyrail")
			writeFile(t, filepath.Join(root, "flyrail.json"), c.manifest)
			for _, directory := range c.directories {
				mkdir(t, filepath.Join(root, directory))
			}
			for _, entry := range c.files {
				writeFile(t, filepath.Join(root, entry.Path), string(entry.Data))
			}
			manifest, err := jsonx.Parse([]byte(c.manifest))
			if err != nil {
				t.Fatal(err)
			}
			identity := must(NewBundleIdentity(fixtureText(t, manifest, "id"), fixtureText(t, manifest, "version")))

			directory := openDir(t, root)
			if diff := cmp.Diff(c.entries, directory.Entries()); diff != "" {
				t.Errorf("directory entries (-want +got):\n%s", diff)
			}
			if directory.ContentDigest() != c.digest || directory.Identity() != identity || directory.SchemaVersion() != 1 {
				t.Errorf("directory digest %s identity %+v schema %d", directory.ContentDigest(), directory.Identity(), directory.SchemaVersion())
			}
			if diff := cmp.Diff([]string{root}, directory.SourceRoots()); diff != "" {
				t.Errorf("directory roots (-want +got):\n%s", diff)
			}

			archive := filepath.Join(parent, "package.zip")
			writeFile(t, archive, string(caseArchive(c, "flyrail")))
			zipped, err := OpenBundleZip(t.Context(), archive, "flyrail")
			if err != nil {
				t.Fatal(err)
			}
			if diff := cmp.Diff(c.entries, zipped.Entries()); diff != "" {
				t.Errorf("archive entries (-want +got):\n%s", diff)
			}
			if zipped.ContentDigest() != c.digest {
				t.Errorf("archive digest %s", zipped.ContentDigest())
			}
			if diff := cmp.Diff([]string{archive}, zipped.SourceRoots()); diff != "" {
				t.Errorf("archive roots (-want +got):\n%s", diff)
			}

			memoryEntries := make([]BundleEntry, 0, len(c.directories)+len(c.files))
			for _, path := range c.directories {
				memoryEntries = append(memoryEntries, dir(path))
			}
			memoryEntries = append(memoryEntries, c.files...)
			remembered := must(NewBundleFromEntries([]byte(c.manifest), memoryEntries))
			if diff := cmp.Diff(c.entries, remembered.Entries()); diff != "" {
				t.Errorf("memory entries (-want +got):\n%s", diff)
			}
			if remembered.ContentDigest() != c.digest || len(remembered.SourceRoots()) != 0 {
				t.Errorf("memory digest %s roots %v", remembered.ContentDigest(), remembered.SourceRoots())
			}

			fsys := fstest.MapFS{"flyrail.json": {Data: []byte(c.manifest)}}
			for _, path := range c.directories {
				fsys[path] = &fstest.MapFile{Mode: fs.ModeDir | 0o755}
			}
			for _, entry := range c.files {
				fsys[entry.Path] = &fstest.MapFile{Data: entry.Data, Mode: 0o755}
			}
			loaded, err := LoadBundleFS(fsys)
			if err != nil {
				t.Fatal(err)
			}
			if diff := cmp.Diff(c.entries, loaded.Entries()); diff != "" {
				t.Errorf("fs entries (-want +got):\n%s", diff)
			}
			if loaded.ContentDigest() != c.digest {
				t.Errorf("fs digest %s", loaded.ContentDigest())
			}

			if c.framing != nil {
				sum := sha256.Sum256(c.framing)
				if hex.EncodeToString(sum[:]) != c.digest {
					t.Errorf("framing digest %x", sum)
				}
			}
		})
	}
}

func TestDigestIsIndependentOfEntryOrderAndCoversEveryField(t *testing.T) {
	entries := []BundleEntry{dir("review"), file("review/SKILL.md", "markdown"), dir("review/empty")}
	reversed := []BundleEntry{entries[2], entries[1], entries[0]}
	if contentDigest(entries) != contentDigest(reversed) {
		t.Fatal("digest depends on entry order")
	}
	executable := file("review/SKILL.md", "markdown")
	executable.Executable = true
	for index, changed := range [][]BundleEntry{
		{entries[0], entries[1]},
		{entries[0], executable, entries[2]},
		{entries[0], file("review/SKILL.md", "markdow"), entries[2]},
		{entries[0], entries[1], file("review/empty", "")},
	} {
		if contentDigest(changed) == contentDigest(entries) {
			t.Errorf("change %d kept the digest", index)
		}
	}
}

func TestSnapshotsContentAndKeepsItAfterTheSourceChanges(t *testing.T) {
	root := tempRoot(t)
	skill := writeBundle(t, root, defaultManifest)
	mkdir(t, filepath.Join(skill, "empty"))
	bundle := openDir(t, root)
	writeFile(t, filepath.Join(skill, "SKILL.md"), "changed")
	if err := os.Remove(filepath.Join(skill, "empty")); err != nil {
		t.Fatal(err)
	}
	writeFile(t, filepath.Join(skill, "later.bin"), "later")
	writeFile(t, filepath.Join(root, "flyrail.json"), "{}")

	if bundle.ID() != "team" || bundle.Version() != "1" {
		t.Fatalf("identity = %s %s", bundle.ID(), bundle.Version())
	}
	skills := bundle.Skills()
	if len(skills) != 1 || skills[0].Name() != "review" || skills[0].Path() != "skills/review" || len(skills[0].Executables()) != 0 {
		t.Fatalf("skills = %+v", skills)
	}
	want := []BundleEntry{dir("review"), file("review/SKILL.md", defaultSkill), dir("review/empty")}
	if diff := cmp.Diff(want, bundle.Entries()); diff != "" {
		t.Fatalf("entries (-want +got):\n%s", diff)
	}
	if contentDigest(bundle.Entries()) != bundle.ContentDigest() {
		t.Fatal("digest does not match the entries")
	}
}

func TestAccessorsReturnCopies(t *testing.T) {
	bundle := must(NewBundleFromEntries([]byte(`{"schema_version":1,"id":"team","version":"1","skills":[{"name":"review","path":"review","executables":["run"]}]}`),
		[]BundleEntry{file("review/SKILL.md", "markdown"), file("review/run", "#!")}))
	entries := bundle.Entries()
	entries[1].Data[0] = 'X'
	bundle.Skills()[0].Executables()[0] = "changed"
	if got := bundle.Entries()[1].Data; string(got) != "markdown" {
		t.Fatalf("entry data = %q", got)
	}
	if got := bundle.Skills()[0].Executables(); got[0] != "run" {
		t.Fatalf("executables = %v", got)
	}
}

func TestSeparatesIdentityFromContent(t *testing.T) {
	root := tempRoot(t)
	skill := writeBundle(t, root, defaultManifest)
	first := openDir(t, root)
	mkdir(t, filepath.Join(root, "other"))
	if err := os.Rename(skill, filepath.Join(root, "other", "review")); err != nil {
		t.Fatal(err)
	}
	if err := os.Remove(filepath.Join(root, "skills")); err != nil {
		t.Fatal(err)
	}
	writeFile(t, filepath.Join(root, "flyrail.json"),
		`{"schema_version":1,"id":"renamed","version":"  older-looking  ","skills":[{"name":"review","path":"other/review"}]}`)
	second := openDir(t, root)
	if second.ID() != "renamed" || second.Version() != "  older-looking  " || second.ContentDigest() != first.ContentDigest() {
		t.Fatalf("second = %s %q %s", second.ID(), second.Version(), second.ContentDigest())
	}
	writeFile(t, filepath.Join(root, "other", "review", "SKILL.md"), "new content")
	if third := openDir(t, root); third.ContentDigest() == second.ContentDigest() {
		t.Fatal("content change kept the digest")
	}
}

func TestTracksEmptyDirectoriesAndDeclaredExecutableIntentOnly(t *testing.T) {
	root := tempRoot(t)
	skill := writeBundle(t, root, defaultManifest)
	initial := openDir(t, root)
	if err := os.Chmod(filepath.Join(skill, "SKILL.md"), 0o700); err != nil {
		t.Fatal(err)
	}
	afterChmod := openDir(t, root)
	if afterChmod.Entries()[1].Executable || afterChmod.ContentDigest() != initial.ContentDigest() {
		t.Fatal("a physical executable bit changed the snapshot")
	}
	mkdir(t, filepath.Join(skill, "empty"))
	withDirectory := openDir(t, root)
	if withDirectory.ContentDigest() == initial.ContentDigest() {
		t.Fatal("an empty directory kept the digest")
	}
	writeFile(t, filepath.Join(root, "flyrail.json"),
		`{"schema_version":1,"id":"team","version":"1","skills":[{"name":"review","path":"skills/review","executables":["SKILL.md"]}]}`)
	executable := openDir(t, root)
	if !executable.Entries()[1].Executable || executable.ContentDigest() == withDirectory.ContentDigest() {
		t.Fatal("declared executable intent was not recorded")
	}
}

func TestIgnoresManifestAndExecutableOrder(t *testing.T) {
	root := tempRoot(t)
	writeFile(t, filepath.Join(root, "flyrail.json"),
		`{"schema_version":1,"id":"team","version":"1","skills":[{"name":"notes","path":"notes"},{"name":"review","path":"skills/review","executables":["tools/run.sh","SKILL.md"]}]}`)
	writeFile(t, filepath.Join(root, "skills", "review", "SKILL.md"), "markdown")
	writeFile(t, filepath.Join(root, "skills", "review", "tools", "run.sh"), "#!/bin/sh\n")
	writeFile(t, filepath.Join(root, "notes", "SKILL.md"), "# Notes\n")
	initial := openDir(t, root)
	writeFile(t, filepath.Join(root, "flyrail.json"),
		`{  "id"  :  "team" , "schema_version" : 1 , "skills" : [{"executables":["SKILL.md","tools/run.sh"],"name":"review","path":"skills/review"},{"name":"notes","path":"notes"}],"version":"1"}`)
	reordered := openDir(t, root)
	if names := []string{reordered.Skills()[0].Name(), reordered.Skills()[1].Name()}; names[0] != "review" || names[1] != "notes" {
		t.Fatalf("skill order = %v", names)
	}
	if diff := cmp.Diff([]string{"SKILL.md", "tools/run.sh"}, reordered.Skills()[0].Executables()); diff != "" {
		t.Fatalf("executables (-want +got):\n%s", diff)
	}
	if reordered.ContentDigest() != initial.ContentDigest() {
		t.Fatal("manifest order changed the digest")
	}
	var notes []BundleEntry
	for _, entry := range initial.Entries() {
		if strings.Split(entry.Path, "/")[0] == "notes" {
			notes = append(notes, entry)
		}
	}
	onlyNotes := tempRoot(t)
	writeFile(t, filepath.Join(onlyNotes, "flyrail.json"),
		`{"schema_version":1,"id":"notes","version":"1","skills":[{"name":"notes","path":"notes"}]}`)
	writeFile(t, filepath.Join(onlyNotes, "notes", "SKILL.md"), "# Notes\n")
	if contentDigest(notes) != openDir(t, onlyNotes).ContentDigest() {
		t.Fatal("per-skill digest differs")
	}
}

func TestRequiresASkillMarkdownFileAndExistingExecutables(t *testing.T) {
	root := tempRoot(t)
	skill := writeBundle(t, root, defaultManifest)
	markdown := filepath.Join(skill, "SKILL.md")
	if err := os.Remove(markdown); err != nil {
		t.Fatal(err)
	}
	const missing = "skill must contain a SKILL.md file: review"
	if got := invalidMessage(t, errorOf(OpenBundle(t.Context(), root))); got != missing {
		t.Fatalf("missing: %q", got)
	}
	writeFile(t, filepath.Join(skill, "skill.md"), "markdown")
	if got := invalidMessage(t, errorOf(OpenBundle(t.Context(), root))); got != missing {
		t.Fatalf("lowercase: %q", got)
	}
	if err := os.Remove(filepath.Join(skill, "skill.md")); err != nil {
		t.Fatal(err)
	}
	mkdir(t, markdown)
	if got := invalidMessage(t, errorOf(OpenBundle(t.Context(), root))); got != missing {
		t.Fatalf("directory: %q", got)
	}
	if err := os.Remove(markdown); err != nil {
		t.Fatal(err)
	}
	writeFile(t, markdown, "markdown")
	mkdir(t, filepath.Join(skill, "empty"))
	for _, target := range []string{"missing.py", "empty", "skill.md"} {
		writeFile(t, filepath.Join(root, "flyrail.json"),
			`{"schema_version":1,"id":"team","version":"1","skills":[{"name":"review","path":"skills/review","executables":["`+target+`"]}]}`)
		if got := invalidMessage(t, errorOf(OpenBundle(t.Context(), root))); got != "every executable must name an existing file: review" {
			t.Errorf("%s: %q", target, got)
		}
	}
}

func TestRejectsMissingAndWrongSourceTypes(t *testing.T) {
	root := tempRoot(t)
	_, err := OpenBundle(t.Context(), filepath.Join(root, "absent"))
	failure, ok := errors.AsType[*BundleError](err)
	if !ok || !errors.Is(err, fs.ErrNotExist) {
		t.Fatalf("absent root: %v", err)
	}
	if want := "[Errno 2] No such file or directory: '" + filepath.Join(root, "absent") + "'"; failure.Message != want {
		t.Fatalf("absent root message %q, want %q", failure.Message, want)
	}
	if got := invalidMessage(t, errorOf(OpenBundle(t.Context(), root))); got != "missing, case-mismatched or ambiguous source path: flyrail.json" {
		t.Fatalf("no manifest: %q", got)
	}
	mkdir(t, filepath.Join(root, "flyrail.json"))
	if got := invalidMessage(t, errorOf(OpenBundle(t.Context(), root))); got != "expected a source file: "+filepath.Join(root, "flyrail.json") {
		t.Fatalf("directory manifest: %q", got)
	}
	if err := os.Remove(filepath.Join(root, "flyrail.json")); err != nil {
		t.Fatal(err)
	}
	skill := writeBundle(t, root, defaultManifest)
	if got := invalidMessage(t, errorOf(OpenBundle(t.Context(), filepath.Join(root, "flyrail.json")))); got != "bundle root must be a directory" {
		t.Fatalf("file root: %q", got)
	}
	if err := os.RemoveAll(skill); err != nil {
		t.Fatal(err)
	}
	writeFile(t, skill, "not a directory")
	if got := invalidMessage(t, errorOf(OpenBundle(t.Context(), root))); got != "skill source must be a directory: skills/review" {
		t.Fatalf("file skill: %q", got)
	}
}

func TestOpenBundleObservesCancellation(t *testing.T) {
	root := tempRoot(t)
	writeBundle(t, root, defaultManifest)
	ctx, cancel := context.WithCancel(t.Context())
	cancel()
	_, err := OpenBundle(ctx, root)
	if _, ok := errors.AsType[*BundleError](err); !ok || !errors.Is(err, context.Canceled) {
		t.Fatalf("err = %v", err)
	}
}

func TestLoadsBundlesFromMemoryManifestsAndValues(t *testing.T) {
	manifest := `{"schema_version": 1, "id": "team", "version": "1", "skills": [{"name": "review", "path": "review"}]}`
	entries := []BundleEntry{file("review/SKILL.md", "markdown")}
	expected := must(NewBundleFromEntries([]byte(manifest), entries))
	if diff := cmp.Diff([]BundleEntry{dir("review"), file("review/SKILL.md", "markdown")}, expected.Entries()); diff != "" {
		t.Fatalf("entries (-want +got):\n%s", diff)
	}
	skills := must(NewArray(must(NewObject(
		ObjectMember{Key: "name", Value: must(NewString("review"))},
		ObjectMember{Key: "path", Value: must(NewString("review"))},
	))))
	record := must(NewObject(
		ObjectMember{Key: "schema_version", Value: Int(1)},
		ObjectMember{Key: "id", Value: must(NewString("team"))},
		ObjectMember{Key: "version", Value: must(NewString("1"))},
		ObjectMember{Key: "skills", Value: skills},
	))
	fromValue := must(NewBundleFromValue(record, entries))
	if fromValue.ContentDigest() != expected.ContentDigest() || fromValue.Identity() != expected.Identity() {
		t.Fatalf("value bundle = %s %+v", fromValue.ContentDigest(), fromValue.Identity())
	}
	dated := must(NewObject(ObjectMember{Key: "schema_version", Value: must(NewDate(2024, 1, 1))}))
	for value, want := range map[string]error{
		"manifest values must be JSON objects, arrays or scalars":                           errorOf(NewBundleFromValue(dated, nil)),
		"expected fields ['id', 'schema_version', 'skills', 'version']; optional fields []": errorOf(NewBundleFromEntries([]byte("{}"), nil)),
		"skill source must be an existing directory: review":                                errorOf(NewBundleFromEntries([]byte(manifest), nil)),
	} {
		if got := invalidMessage(t, want); got != value {
			t.Errorf("got %q, want %q", got, value)
		}
	}
	if _, err := NewBundleFromValue(nil, nil); err == nil {
		t.Fatal("nil manifest value was accepted")
	}
}

func TestMemoryEntriesAreValidatedAndSnapshotted(t *testing.T) {
	manifest := []byte(`{"schema_version":1,"id":"team","version":"1","skills":[{"name":"review","path":"review"}]}`)
	cases := map[string][]BundleEntry{
		"entry path must be an unambiguous relative path":    {file("../escape", "")},
		"a directory cannot have executable file intent":     {{Path: "review", Dir: true, Executable: true}},
		"a directory cannot have file data":                  {{Path: "review", Dir: true, Data: []byte{}}},
		"duplicate entry path":                               {file("review/SKILL.md", ""), file("review/SKILL.md", "")},
		"portable entry path collision":                      {file("review/a", ""), file("review/A", "")},
		"entry file/directory collision":                     {file("review/a", ""), file("review/a/b", "")},
		"skill source must be an existing directory: review": {file("review", "")},
	}
	for want, entries := range cases {
		if got := invalidMessage(t, errorOf(NewBundleFromEntries(manifest, entries))); got != want {
			t.Errorf("got %q, want %q", got, want)
		}
	}
	data := []byte("markdown")
	bundle := must(NewBundleFromEntries(manifest, []BundleEntry{{Path: "review/SKILL.md", Data: data}}))
	data[0] = 'X'
	if got := bundle.Entries()[1].Data; string(got) != "markdown" {
		t.Fatalf("entry data = %q", got)
	}
}

func TestLoadBundleFSIgnoresModesAndRefusesLinks(t *testing.T) {
	manifest := []byte(`{"schema_version":1,"id":"team","version":"1","skills":[{"name":"review","path":"skills/review","executables":["run.sh"]}]}`)
	fsys := fstest.MapFS{
		"flyrail.json":             {Data: manifest, Mode: 0o755},
		"skills/review/SKILL.md":   {Data: []byte("markdown"), Mode: 0o755},
		"skills/review/run.sh":     {Data: []byte("#!/bin/sh\n"), Mode: 0o644},
		"skills/review/data/x.bin": {Data: []byte{0, 1}, Mode: 0o700},
	}
	loaded, err := LoadBundleFS(fsys)
	if err != nil {
		t.Fatal(err)
	}
	executable := file("review/run.sh", "#!/bin/sh\n")
	executable.Executable = true
	want := []BundleEntry{dir("review"), file("review/SKILL.md", "markdown"), dir("review/data"), file("review/data/x.bin", "\x00\x01"), executable}
	if diff := cmp.Diff(want, loaded.Entries()); diff != "" {
		t.Fatalf("entries (-want +got):\n%s", diff)
	}
	if loaded.SourceRoots() != nil || loaded.ContentDigest() != contentDigest(want) {
		t.Fatalf("roots %v digest %s", loaded.SourceRoots(), loaded.ContentDigest())
	}
	fsys["skills/review/link"] = &fstest.MapFile{Data: []byte("SKILL.md"), Mode: fs.ModeSymlink | 0o777}
	if got := invalidMessage(t, errorOf(LoadBundleFS(fsys))); got != "source symlinks and special files are not supported: skills/review/link" {
		t.Fatalf("symlink: %q", got)
	}
	delete(fsys, "skills/review/link")
	fsys["Skills/other"] = &fstest.MapFile{Data: []byte("x")}
	if got := invalidMessage(t, errorOf(LoadBundleFS(fsys))); got != "portable source name collision: skills" {
		t.Fatalf("collision: %q", got)
	}
	delete(fsys, "Skills/other")
	delete(fsys, "flyrail.json")
	fsys["FLYRAIL.json"] = &fstest.MapFile{Data: manifest}
	if got := invalidMessage(t, errorOf(LoadBundleFS(fsys))); got != "missing, case-mismatched or ambiguous source path: flyrail.json" {
		t.Fatalf("case mismatch: %q", got)
	}
}

func TestRejectsNonportableZipResources(t *testing.T) {
	root := tempRoot(t)
	archive := filepath.Join(root, "package.zip")
	writeFile(t, archive, string(ziptest.Write(ziptest.File("flyrail/x", nil))))
	for _, resource := range []string{"", ".", "../flyrail", "/flyrail", "data\\flyrail", "CON"} {
		if got := invalidMessage(t, errorOf(OpenBundleZip(t.Context(), archive, resource))); !strings.HasPrefix(got, "archive resource ") {
			t.Errorf("%q: %q", resource, got)
		} else if resource == "" && got != "archive resource must be an unambiguous relative path" {
			t.Errorf("empty resource: %q", got)
		}
	}
}

func TestOpenBundleZipReportsArchiveErrorsWithThePath(t *testing.T) {
	root := tempRoot(t)
	archive := filepath.Join(root, "package.zip")
	writeFile(t, archive, "not a zip")
	_, err := OpenBundleZip(t.Context(), archive, "flyrail")
	failure, ok := errors.AsType[*BundleError](err)
	pathError, isPath := errors.AsType[*fs.PathError](err)
	if !ok || !isPath || failure.Message != "archive is not a zip file" || pathError.Path != archive {
		t.Fatalf("err = %#v", err)
	}
}

func TestOpenBundleZipRefusesOversizedArchivesBeforeReading(t *testing.T) {
	root := tempRoot(t)
	archive := filepath.Join(root, "package.zip")
	writeFile(t, archive, "")
	if err := os.Truncate(archive, 256<<20+1); err != nil {
		t.Fatal(err)
	}
	_, err := OpenBundleZip(t.Context(), archive, "flyrail")
	if failure, ok := errors.AsType[*BundleError](err); !ok || failure.Message != "archive exceeds the byte limit" {
		t.Fatalf("err = %v", err)
	}
}
