package main

import (
	"encoding/hex"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/google/go-cmp/cmp"
)

func decodedRecords(t *testing.T, stdout string) map[string]string {
	t.Helper()
	records := map[string]string{}
	for line := range strings.SplitSeq(strings.TrimSuffix(stdout, "\n"), "\n") {
		fields := strings.Split(line, "\t")
		if len(fields) != 4 {
			t.Fatalf("unexpected record %q", line)
		}
		value, err := hex.DecodeString(fields[3])
		if err != nil {
			t.Fatal(err)
		}
		records[fields[1]+"/"+fields[2]] = string(value)
	}
	return records
}

func TestDifferentialEmitsLifecycleRecordsWithNormalizedPaths(t *testing.T) {
	dir := tempRoot(t)
	corpus := writeCorpus(t, dir, "lifecycle\tabsent-file\tfile\t-\tone\ttwo\tconflict\n")

	code, stdout, stderr := runDriver("differential", "--corpus", corpus, "--root", filepath.Join(dir, "work"))

	if code != 0 {
		t.Fatalf("exit %d, stderr %q", code, stderr)
	}
	records := decodedRecords(t, stdout)
	payloads := map[string]string{
		"absent-file/1-create-payloads": `[["$ROOT/owned","6f6e65"]]`,
		"absent-file/3-update-payloads": `[["$ROOT/owned","74776f"]]`,
		"absent-file/6-remove-payloads": `[]`,
	}
	for key, want := range payloads {
		if diff := cmp.Diff(want, records[key]); diff != "" {
			t.Errorf("%s (-want +got):\n%s", key, diff)
		}
	}
	for key, want := range map[string]string{
		"absent-file/1-create-result":    `"status":"applied"`,
		"absent-file/2-unchanged-result": `"status":"unchanged"`,
		"absent-file/4-recover-result":   `"status":"unchanged"`,
		"absent-file/6-remove-result":    `"status":"applied"`,
		"absent-file/5-inspect":          `{"current":true,"pending":false,"revision":["`,
	} {
		if !strings.Contains(records[key], want) {
			t.Errorf("%s = %s, want it to contain %s", key, records[key], want)
		}
	}
	receipt := records["absent-file/1-create-state-resource/$ROOT/owned/receipt"]
	for _, want := range []string{`"transaction_id":"00000000000000000000000000000000"`, `"path":"$ROOT/owned"`, `"device":0`} {
		if !strings.Contains(receipt, want) {
			t.Errorf("receipt %s does not contain %s", receipt, want)
		}
	}
	if !strings.HasSuffix(receipt, "}\n") {
		t.Errorf("receipt %q does not end in a newline", receipt)
	}
}

func TestDifferentialRejectsInvalidLifecycleRows(t *testing.T) {
	dir := tempRoot(t)
	for _, row := range []string{
		"lifecycle\tshort\tfile\t-\tone\ttwo",
		"lifecycle\thex\tfile\tzz\tone\ttwo\tconflict",
		"lifecycle\tacquire\tfile\t-\tone\ttwo\tsteal",
	} {
		corpus := writeCorpus(t, dir, row+"\n")
		code, _, stderr := runDriver("differential", "--corpus", corpus, "--root", filepath.Join(dir, "work-"+strings.Split(row, "\t")[1]))
		if code != 1 || stderr == "" {
			t.Errorf("%q: exit %d, stderr %q", row, code, stderr)
		}
	}
}

func TestZeroedTransactionIDsCoverStagedAndBesideDirectories(t *testing.T) {
	digest := strings.Repeat("a", 64)
	identifier := strings.Repeat("b", 32)
	zeros := strings.Repeat("0", 32)
	cases := map[string]string{
		"/x/" + digest + "-" + identifier:                        "/x/" + digest + "-" + zeros,
		"/x/.flyrail-" + digest + "-" + identifier + ".txn/a":    "/x/.flyrail-" + digest + "-" + zeros + ".txn/a",
		"/x/.flyrail-" + digest + "-" + identifier + ".txnextra": "/x/.flyrail-" + digest + "-" + zeros + ".txnextra",
		"/x/" + digest + "-" + identifier + ".txn":               "/x/" + digest + "-" + zeros + ".txn",
		"/x/" + digest + "-" + identifier + "\n":                 "/x/" + digest + "-" + zeros + "\n",
		"/x/" + digest + "-" + identifier + "\n/y":               "/x/" + digest + "-" + identifier + "\n/y",
		"/x/" + digest + "-" + identifier + "x":                  "/x/" + digest + "-" + identifier + "x",
		"/root/" + digest + "-" + identifier + "/" + identifier:  "$ROOT/" + digest + "-" + zeros + "/" + identifier,
		"/x/" + digest + "-short":                                "/x/" + digest + "-short",
		"/root/AGENTS.md":                                        "$ROOT/AGENTS.md",
	}
	for path, want := range cases {
		if got := relativePath(path, "/root"); got != want {
			t.Errorf("relativePath(%q) = %q, want %q", path, got, want)
		}
	}
}

func writeText(t *testing.T, path, text string) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(text), 0o600); err != nil {
		t.Fatal(err)
	}
}

func TestPayloadsAreOrderedByPathComponents(t *testing.T) {
	root := tempRoot(t)
	writeText(t, filepath.Join(root, "a.b"), "1")
	writeText(t, filepath.Join(root, "a", "c"), "2")
	got, err := payloadRecord(root)
	if err != nil {
		t.Fatal(err)
	}
	if diff := cmp.Diff(`[["$ROOT/a/c","32"],["$ROOT/a.b","31"]]`, string(got)); diff != "" {
		t.Errorf("payloads (-want +got):\n%s", diff)
	}
}

func TestPayloadsFollowFileLinksAndSkipOtherLinks(t *testing.T) {
	root := tempRoot(t)
	writeText(t, filepath.Join(root, "real", "f"), "x")
	for link, target := range map[string]string{"dirlink": "real", "dangling": "missing", "filelink": filepath.Join("real", "f")} {
		if err := os.Symlink(target, filepath.Join(root, link)); err != nil {
			t.Fatal(err)
		}
	}
	got, err := payloadRecord(root)
	if err != nil {
		t.Fatal(err)
	}
	if diff := cmp.Diff(`[["$ROOT/filelink","78"],["$ROOT/real/f","78"]]`, string(got)); diff != "" {
		t.Errorf("payloads (-want +got):\n%s", diff)
	}
}

func TestSharedSectionsAreRenderedInReverseInputOrder(t *testing.T) {
	for kind, want := range map[string][]string{"shared": {"zeta", "alpha"}, "alias": {"alpha", "zeta"}} {
		artifacts, err := pairedArtifacts(filepath.Join(tempRoot(t), "AGENTS.md"), kind, "text")
		if err != nil {
			t.Fatal(err)
		}
		ids := make([]string, 0, len(artifacts))
		for _, artifact := range artifacts {
			ids = append(ids, artifact.ID())
		}
		if diff := cmp.Diff(want, ids); diff != "" {
			t.Errorf("%s: ids (-want +got):\n%s", kind, diff)
		}
	}
}
