package main

import (
	"bytes"
	"encoding/hex"
	"os"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
	"testing"

	"github.com/google/go-cmp/cmp"

	"github.com/nielsmadan/flyrail/go/flyrail"
)

var ownedRecord = regexp.MustCompile(`^\d+-ownership-\d+$|^\d-[a-z]+-state-`)

func tempRoot(t *testing.T) string {
	t.Helper()
	root, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	return root
}

func writeCorpus(t *testing.T, dir, text string) string {
	t.Helper()
	path := filepath.Join(dir, "corpus.tsv")
	if err := os.WriteFile(path, []byte(text), 0o600); err != nil {
		t.Fatal(err)
	}
	return path
}

func runDriver(args ...string) (code int, stdout, stderr string) {
	var out, errOut bytes.Buffer
	code = run(args, strings.NewReader(""), &out, &errOut)
	return code, out.String(), errOut.String()
}

func hexText(text string) string {
	return hex.EncodeToString([]byte(text))
}

func TestDifferentialEmitsCanonicalValueRecords(t *testing.T) {
	dir := tempRoot(t)
	corpus := writeCorpus(t, dir, strings.Join([]string{
		"# header",
		"value\tvalue-00\t" + hexText(`["null"]`),
		"value\tvalue-01\t" + hexText(`["object", [["b", ["integer", "-5"]], ["a", ["string", "x"]]]]`),
		"",
	}, "\n"))
	root := filepath.Join(dir, "work")

	code, stdout, stderr := runDriver("differential", "--root", root, "--corpus", corpus)

	if code != 0 {
		t.Fatalf("exit %d, stderr %q", code, stderr)
	}
	want := "value\tvalue-00\tsemantic\t" + hexText(`["null"]`) + "\n" +
		"value\tvalue-01\tsemantic\t" + hexText(`["object",[["a",["string","x"]],["b",["integer","-5"]]]]`) + "\n"
	if diff := cmp.Diff(want, stdout); diff != "" {
		t.Errorf("records (-want +got):\n%s", diff)
	}
	if info, err := os.Stat(root); err != nil || !info.IsDir() {
		t.Errorf("root directory: %v, %v", info, err)
	}
}

func TestDifferentialEmitsBundleDigestAndCanonicalModel(t *testing.T) {
	dir := tempRoot(t)
	manifest := `{"schema_version":2,"id":"team","version":"1","artifacts":[{"kind":"instruction","id":"guide","text":"Hello"}]}`
	corpus := writeCorpus(t, dir, "bundle\tbundle-00\t2\t"+hexText(manifest)+"\t1\tunused\t-\t0\n")

	code, stdout, stderr := runDriver("differential", "--corpus", corpus, "--root", filepath.Join(dir, "work"))

	if code != 0 {
		t.Fatalf("exit %d, stderr %q", code, stderr)
	}
	canonical := `["array",[["array",[["object",[["fields",["object",[["id",["string","guide"]],["text",["string","Hello"]]]]],` +
		`["type",["string","InstructionArtifact"]]]]]],["array",[]],["array",[]]]]`
	want := "bundle\tbundle-00\tdigest\t" + hexText("79d34d349d9f79d264aa80eb99e1e3887664cf0e5674bfa87f1f47379ddbb2da") + "\n" +
		"bundle\tbundle-00\tcanonical\t" + hexText(canonical) + "\n"
	if diff := cmp.Diff(want, stdout); diff != "" {
		t.Errorf("records (-want +got):\n%s", diff)
	}
}

func TestDifferentialCoversEveryCorpusRow(t *testing.T) {
	path := filepath.Join("..", "..", "..", "..", "spec", "fixtures", "differential.tsv")
	data, err := os.ReadFile(path)
	if os.IsNotExist(err) {
		if os.Getenv("FLYRAIL_REQUIRE_FIXTURES") == "1" {
			t.Fatalf("fixture %s is required but missing", path)
		}
		t.Skipf("skipping: spec fixture %s is not available", path)
	}
	if err != nil {
		t.Fatal(err)
	}
	var names []string
	for line := range strings.SplitSeq(string(data), "\n") {
		switch fields := strings.Split(line, "\t"); fields[0] {
		case "value":
			names = append(names, "value/"+fields[1]+"/semantic")
		case "bundle":
			names = append(names, "bundle/"+fields[1]+"/digest", "bundle/"+fields[1]+"/canonical")
		case "render":
			for _, field := range []string{"digest", "canonical", "context", "claim-0", "claim-1"} {
				names = append(names, "render/"+fields[1]+"/"+field)
			}
		case "edit":
			for step := 1; step <= (len(fields)-5)/3; step++ {
				prefix := "edit/" + fields[1] + "/" + strconv.Itoa(step)
				names = append(names, prefix+"-bytes", prefix+"-status", prefix+"-provenance", prefix+"-ownership-count")
			}
		case "lifecycle":
			for _, step := range []string{"1-create", "2-unchanged", "3-update", "4-recover"} {
				names = append(names, "lifecycle/"+fields[1]+"/"+step+"-result", "lifecycle/"+fields[1]+"/"+step+"-payloads")
			}
			names = append(names, "lifecycle/"+fields[1]+"/5-inspect", "lifecycle/"+fields[1]+"/6-remove-result", "lifecycle/"+fields[1]+"/6-remove-payloads")
		}
	}

	code, stdout, stderr := runDriver("differential", "--corpus", path, "--root", filepath.Join(tempRoot(t), "work"))

	if code != 0 {
		t.Fatalf("exit %d, stderr %q", code, stderr)
	}
	var emitted []string
	for line := range strings.SplitSeq(strings.TrimSuffix(stdout, "\n"), "\n") {
		fields := strings.Split(line, "\t")
		if len(fields) != 4 {
			t.Fatalf("unexpected record %q", line)
		}
		if !ownedRecord.MatchString(fields[2]) {
			emitted = append(emitted, fields[0]+"/"+fields[1]+"/"+fields[2])
		}
	}
	if diff := cmp.Diff(names, emitted); diff != "" {
		t.Errorf("cases (-corpus +emitted):\n%s", diff)
	}
}

func TestDifferentialEmitsRenderRecordsBelowACaseDirectory(t *testing.T) {
	dir := tempRoot(t)
	corpus := writeCorpus(t, dir, "render\trender-03\tdoc-03.txt\t"+hexText("plain")+"\t42\tfile-first\tzeta-first\n")
	root := filepath.Join(dir, "work")

	code, stdout, stderr := runDriver("differential", "--corpus", corpus, "--root", root)

	if code != 0 {
		t.Fatalf("exit %d, stderr %q", code, stderr)
	}
	lines := strings.Split(strings.TrimSuffix(stdout, "\n"), "\n")
	if len(lines) != 5 {
		t.Fatalf("records %q", stdout)
	}
	digest := strings.Split(lines[0], "\t")
	raw, err := hex.DecodeString(digest[3])
	if digest[2] != "digest" || err != nil || len(raw) != 64 {
		t.Errorf("digest record %q", lines[0])
	}
	canonical := strings.Split(lines[1], "\t")
	model, err := hex.DecodeString(canonical[3])
	if canonical[2] != "canonical" || err != nil || !strings.Contains(string(model), `["string","`+root+`/render-03/doc-03.txt"]`) {
		t.Errorf("canonical record %q", model)
	}
	context := `["array",[["array",[["string","alpha"],["string","Straße"]]],["array",[["string","zeta"],["string","42"]]]]]`
	if want := "render\trender-03\tcontext\t" + hexText(context); lines[2] != want {
		t.Errorf("context record %q, want %q", lines[2], want)
	}
	if info, err := os.Stat(filepath.Join(root, "render-03")); err != nil || !info.IsDir() {
		t.Errorf("case directory: %v, %v", info, err)
	}
	authority, err := flyrail.NewResourceAuthority(t.Context(), filepath.Join(root, "render-03", "doc-03.txt"))
	if err != nil {
		t.Fatal(err)
	}
	claim, err := flyrail.NewClaim(flyrail.OwnershipFile, flyrail.WholeSelector{})
	if err != nil {
		t.Fatal(err)
	}
	identity, err := claim.Identity(authority)
	if err != nil {
		t.Fatal(err)
	}
	if want := "render\trender-03\tclaim-0\t" + hexText(identity); lines[3] != want {
		t.Errorf("file claim record %q, want %q", lines[3], want)
	}
	if fields := strings.Split(lines[4], "\t"); fields[2] != "claim-1" || len(fields[3]) != 128 {
		t.Errorf("section claim record %q", lines[4])
	}
}

func TestDifferentialRejectsInvalidInput(t *testing.T) {
	cases := []struct {
		name    string
		corpus  string
		message string
	}{
		{"unknown category", "matrix\tcase-00\t00\n", "unknown differential category"},
		{"short row", "value\n", "invalid differential row"},
		{"value columns", "value\tvalue-00\n", "invalid differential value row"},
		{"hex", "value\tvalue-00\tzz\n", "invalid hexadecimal data"},
		{"invalid UTF-8", "value\tvalue-00\t\xff\n", "invalid differential corpus"},
		{"semantic value", "value\tvalue-00\t" + hexText(`["integer",1]`) + "\n", "semantic scalar payload has the wrong type"},
		{"bundle columns", "bundle\tbundle-00\t2\t00\n", "invalid differential bundle row"},
		{"bundle entry count", "bundle\tbundle-00\t2\t00\t1\tpath\t-\n", "bundle-00: invalid bundle entry count"},
		{"bundle entry data", "bundle\tbundle-00\t2\t00\t1\tpath\tzz\t0\n", "invalid hexadecimal data"},
		{"render columns", "render\trender-00\tdoc.txt\t00\t1\tfile-first\n", "invalid differential render row"},
		{"render payload", "render\trender-00\tdoc.txt\tzz\t1\tfile-first\talpha-first\n", "invalid hexadecimal data"},
		{"bundle manifest", "bundle\tbundle-00\t2\t" + hexText(`{"schema_version":2}`) + "\t0\n",
			"expected fields ['artifacts', 'id', 'schema_version', 'version']; optional fields ['assets', 'dependencies']"},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			dir := tempRoot(t)
			code, stdout, stderr := runDriver("differential",
				"--corpus", writeCorpus(t, dir, tc.corpus), "--root", filepath.Join(dir, "work"))
			if code != 1 || stdout != "" || stderr != tc.message+"\n" {
				t.Errorf("got exit %d, stdout %q, stderr %q; want exit 1 and %q", code, stdout, stderr, tc.message)
			}
		})
	}
}

func TestDifferentialRejectsOversizeCorpusBeforeCreatingRoot(t *testing.T) {
	dir := tempRoot(t)
	path := filepath.Join(dir, "oversize.tsv")
	file, err := os.Create(path)
	if err != nil {
		t.Fatal(err)
	}
	if err := file.Truncate(maxCorpusBytes + 1); err != nil {
		t.Fatal(err)
	}
	if err := file.Close(); err != nil {
		t.Fatal(err)
	}
	root := filepath.Join(dir, "work")

	code, stdout, stderr := runDriver("differential", "--corpus", path, "--root", root)

	if code != 1 || stdout != "" || stderr != "invalid differential corpus\n" {
		t.Errorf("got exit %d, stdout %q, stderr %q", code, stdout, stderr)
	}
	if _, err := os.Lstat(root); !os.IsNotExist(err) {
		t.Errorf("root was created for a rejected corpus: %v", err)
	}
}

func TestDifferentialRefusesExistingRoot(t *testing.T) {
	dir := tempRoot(t)
	corpus := writeCorpus(t, dir, "value\tvalue-00\t"+hexText(`["null"]`)+"\n")

	code, stdout, stderr := runDriver("differential", "--corpus", corpus, "--root", dir)

	if code != 1 || stdout != "" || !strings.Contains(stderr, "file exists") {
		t.Errorf("got exit %d, stdout %q, stderr %q", code, stdout, stderr)
	}
}

func TestDifferentialRejectsMalformedOptions(t *testing.T) {
	cases := [][]string{
		{},
		{"--corpus", "a.tsv"},
		{"--corpus", "a.tsv", "--root"},
		{"--corpus", "a.tsv", "--corpus", "b.tsv"},
		{"--corpus", "a.tsv", "--root", "r", "--extra", "x"},
	}
	for _, args := range cases {
		code, stdout, stderr := runDriver(append([]string{"differential"}, args...)...)
		if code != 1 || stdout != "" || stderr != errUsage.Error()+"\n" {
			t.Errorf("%q: got exit %d, stdout %q, stderr %q", args, code, stdout, stderr)
		}
	}
}

func TestUnknownCommandPrintsErrorRecord(t *testing.T) {
	code, stdout, stderr := runDriver("unknown")

	want := `{"error":{"code":null,"message":"` + usage + `","path":null}}` + "\n"
	if code != 1 || stdout != want || stderr != "" {
		t.Errorf("got exit %d, stdout %q, stderr %q", code, stdout, stderr)
	}
}
