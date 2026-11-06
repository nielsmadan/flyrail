package main

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/google/go-cmp/cmp"
)

func TestDifferentialEmitsEditRecordsForEachStep(t *testing.T) {
	dir := tempRoot(t)
	corpus := writeCorpus(t, dir, "edit\ttoml-small\ttoml\t"+hexText("a = 1")+"\t2\tset\tone\tconflict\tretire\t-\t-\n")

	code, stdout, stderr := runDriver("differential", "--corpus", corpus, "--root", filepath.Join(dir, "work"))

	if code != 0 {
		t.Fatalf("exit %d, stderr %q", code, stderr)
	}
	want := "edit\ttoml-small\t1-bytes\t61203d20310a746f6f6c203d20226f6e65220a\n" +
		"edit\ttoml-small\t1-status\t63726561746564\n" +
		"edit\ttoml-small\t1-provenance\t7b226669656c6473223a7b22636f6e7461696e657273223a5b5d2c22656d7074795f646f63756d656e74223a6e756c6c2c22736368656d615f656d7074795f646f63756d656e74223a6e756c6c2c22736368656d615f6669656c6473223a5b5d2c22747261696c696e675f736570617261746f7273223a5b2262356263316666643930393132666231386265663665376438303930393139326337613634393238393633323031353664363766626166313034633635343461225d7d2c2274797065223a22446f63756d656e7450726f76656e616e6365227d0a\n" +
		"edit\ttoml-small\t1-ownership-count\t31\n" +
		"edit\ttoml-small\t1-ownership-0\t7b226669656c6473223a7b22626173656c696e65223a6e756c6c2c22636c61696d223a7b226669656c6473223a7b226b696e64223a7b22656e756d223a224f776e657273686970222c2276616c7565223a2273747275637475726564227d2c2273656c6563746f72223a7b226669656c6473223a7b227061727473223a5b7b226669656c6473223a7b226e616d65223a22746f6f6c227d2c2274797065223a224b6579227d5d7d2c2274797065223a2253656c6563746f72227d7d2c2274797065223a22436c61696d227d2c22646973706f736974696f6e223a7b22656e756d223a22446973706f736974696f6e222c2276616c7565223a2263726561746564227d2c22666f726d6174223a7b22656e756d223a22446f63756d656e74466f726d6174222c2276616c7565223a22746f6d6c227d2c22696e7374616c6c6564223a7b226669656c6473223a7b226c61796f7574223a7b226279746573223a22227d2c226f776e65645f73796e746178223a7b226279746573223a2232323666366536353232227d2c22706f736974696f6e223a6e756c6c2c2273796e746178223a7b226279746573223a2237363631366337353635323033643230323236663665363532323061227d2c2276616c7565223a7b2276616c7565223a5b22737472696e67222c226f6e65225d7d7d2c2274797065223a2253656c656374696f6e536e617073686f74227d2c22736570617261746f72223a7b226279746573223a22227d7d2c2274797065223a224f776e656453656c656374696f6e227d0a\n" +
		"edit\ttoml-small\t2-bytes\t61203d2031\n" +
		"edit\ttoml-small\t2-status\t72656d6f766564\n" +
		"edit\ttoml-small\t2-provenance\t7b226669656c6473223a7b22636f6e7461696e657273223a5b5d2c22656d7074795f646f63756d656e74223a6e756c6c2c22736368656d615f656d7074795f646f63756d656e74223a6e756c6c2c22736368656d615f6669656c6473223a5b5d2c22747261696c696e675f736570617261746f7273223a5b5d7d2c2274797065223a22446f63756d656e7450726f76656e616e6365227d0a\n" +
		"edit\ttoml-small\t2-ownership-count\t30\n"
	if diff := cmp.Diff(want, stdout); diff != "" {
		t.Errorf("records (-want +got):\n%s", diff)
	}
}

func TestDifferentialRejectsInvalidEditRows(t *testing.T) {
	dir := tempRoot(t)
	for _, row := range []string{
		"edit\tshort\ttoml",
		"edit\tcount\ttoml\t-\t2\tset\tone\tconflict",
		"edit\tfirst\ttoml\t-\t1\tretire\t-\t-",
		"edit\thex\ttoml\tzz\t1\tset\tone\tconflict",
		"edit\tacquire\ttoml\t-\t1\tset\tone\tsteal",
	} {
		corpus := writeCorpus(t, dir, row+"\n")
		code, _, stderr := runDriver("differential", "--corpus", corpus, "--root", filepath.Join(dir, "work-"+strings.Split(row, "\t")[1]))
		if code != 1 || stderr == "" {
			t.Errorf("%q: exit %d, stderr %q", row, code, stderr)
		}
	}
}

func TestEditsCommandPrintsReferenceEditingRecords(t *testing.T) {
	dir := tempRoot(t)
	corpus := filepath.Join(dir, "cases.json")
	cases := `[{"name":"small","format":"toml","before":"a = 1","steps":[{"action":"set","selector":["x"],"value":{"k":[1,2]}},` +
		`{"action":"retire","selector":["x"]}]},{"name":"bad","format":"toml","before":"x=","steps":[{"action":"set","selector":["x"],"value":1}]}]`
	if err := os.WriteFile(corpus, []byte(cases), 0o600); err != nil {
		t.Fatal(err)
	}

	code, stdout, stderr := runDriver("edits", "--corpus", corpus)

	if code != 0 {
		t.Fatalf("exit %d, stderr %q", code, stderr)
	}
	want := "small\t0\t{\"after\":\"61203d20310a5b785d0a6b203d205b312c20325d0a\",\"applicable\":true,\"before\":\"61203d2031\",\"observations\":[{\"claim\":{\"kind\":\"structured\",\"selector\":[\"x\"]},\"current\":null,\"message\":\"\",\"status\":\"created\"}],\"ownership\":[{\"baseline\":null,\"claim\":{\"kind\":\"structured\",\"selector\":[\"x\"]},\"disposition\":\"created\",\"format\":\"toml\",\"installed\":{\"layout\":\"\",\"owned_syntax\":\"6b203d205b312c20325d0a\",\"position\":null,\"syntax\":\"5b76616c75655d0a6b203d205b312c20325d0a\",\"value\":{\"semantic\":\"5b226f626a656374222c5b5b226b222c5b226172726179222c5b5b22696e7465676572222c2231225d2c5b22696e7465676572222c2232225d5d5d5d5d5d\"}},\"separator\":\"\"}],\"provenance\":{\"containers\":[],\"empty_document\":null,\"schema_empty_document\":null,\"schema_fields\":[],\"trailing_separators\":[\"b5bc1ffd90912fb18bef6e7d80909192c7a6492896320156d67fbaf104c6544a\"]}}\n" +
		"small\t1\t{\"after\":\"61203d2031\",\"applicable\":true,\"before\":\"61203d20310a5b785d0a6b203d205b312c20325d0a\",\"observations\":[{\"claim\":{\"kind\":\"structured\",\"selector\":[\"x\"]},\"current\":{\"layout\":\"\",\"owned_syntax\":\"6b203d205b312c20325d0a\",\"position\":null,\"syntax\":\"5b76616c75655d0a6b203d205b312c20325d0a\",\"value\":{\"semantic\":\"5b226f626a656374222c5b5b226b222c5b226172726179222c5b5b22696e7465676572222c2231225d2c5b22696e7465676572222c2232225d5d5d5d5d5d\"}},\"message\":\"\",\"status\":\"removed\"}],\"ownership\":[],\"provenance\":{\"containers\":[],\"empty_document\":null,\"schema_empty_document\":null,\"schema_fields\":[],\"trailing_separators\":[]}}\n" +
		"bad\t0\t{\"error\":\"Unexpected character: '\\\\x00' at line 1 col 2\"}\n"
	if diff := cmp.Diff(want, stdout); diff != "" {
		t.Errorf("records (-want +got):\n%s", diff)
	}
}

func TestEditsCommandRejectsMalformedOptions(t *testing.T) {
	for _, args := range [][]string{{"edits"}, {"edits", "--root", "x"}, {"edits", "--corpus"}} {
		code, stdout, stderr := runDriver(args...)
		if code != 1 || stdout != "" || stderr != errEditsUsage.Error()+"\n" {
			t.Errorf("%q: got exit %d, stdout %q, stderr %q", args, code, stdout, stderr)
		}
	}
}
