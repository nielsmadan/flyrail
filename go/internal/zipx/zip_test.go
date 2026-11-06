package zipx

import (
	"bytes"
	"encoding/binary"
	"errors"
	"hash/crc32"
	"strings"
	"testing"

	"github.com/nielsmadan/flyrail/go/internal/zipx/ziptest"
)

func open(t *testing.T, data []byte) *Archive {
	t.Helper()
	archive, err := Open(data, DefaultLimits())
	if err != nil {
		t.Fatalf("Open: %v", err)
	}
	return archive
}

func reason(t *testing.T, err error) string {
	t.Helper()
	failure, ok := errors.AsType[*Error](err)
	if !ok {
		t.Fatalf("error = %v, want *zipx.Error", err)
	}
	return failure.Reason
}

func TestRoundTripsStoredMembersWithDirectoryEntries(t *testing.T) {
	archive := open(t, ziptest.Write(
		ziptest.Directory("outer"),
		ziptest.Directory("outer/a"),
		ziptest.File("outer/a/SKILL.md", []byte("markdown")),
	))
	members := archive.Members()
	if len(members) != 3 || members[0].Filename != "outer/" || !members[0].IsDir || members[2].IsDir {
		t.Fatalf("members = %+v", members)
	}
	data, err := archive.Read(2)
	if err != nil || string(data) != "markdown" {
		t.Fatalf("Read = %q, %v", data, err)
	}
}

func TestKeepsUTF8NamesAndDecodesCP437WithoutTheFlag(t *testing.T) {
	utf8Name := ziptest.Blank()
	utf8Name.Name = "café.txt"
	utf8Name.FlagBits = utf8Flag
	highByte := ziptest.Blank()
	highByte.RawName = []byte("caf\xe9.txt")
	unflagged := ziptest.Blank()
	unflagged.RawName = []byte("café.txt")
	members := open(t, ziptest.Write(utf8Name, highByte, unflagged)).Members()
	want := []string{"café.txt", "cafΘ.txt", "caf├⌐.txt"}
	for index, name := range want {
		if members[index].Filename != name {
			t.Errorf("member %d = %q, want %q", index, members[index].Filename, name)
		}
	}
}

func TestTruncatesFilenamesAtTheFirstNUL(t *testing.T) {
	member := open(t, ziptest.Write(ziptest.File("a/name\x00hidden", nil))).Members()[0]
	if member.Filename != "a/name" || member.OrigFilename != "a/name\x00hidden" {
		t.Fatalf("member = %q / %q", member.Filename, member.OrigFilename)
	}
}

func TestInflatesDeflatedPayloads(t *testing.T) {
	text := strings.Repeat("hello ", 32)
	spec := ziptest.File("text.txt", []byte(text))
	spec.Method = 8
	archive := open(t, ziptest.Write(spec))
	if archive.members[0].compressedSize >= uint64(len(text)) {
		t.Fatalf("compressed size = %d", archive.members[0].compressedSize)
	}
	data, err := archive.Read(0)
	if err != nil || string(data) != text {
		t.Fatalf("Read = %q, %v", data, err)
	}
}

func dropLast(data []byte) []byte {
	return data[:len(data)-1]
}

func TestRejectsCorruptAndTruncatedArchives(t *testing.T) {
	for name, data := range map[string][]byte{
		"text":      []byte("not a zip"),
		"bare end":  []byte("PK\x05\x06"),
		"truncated": dropLast(ziptest.Write(ziptest.File("a.txt", []byte("data")))),
	} {
		if _, err := Open(data, DefaultLimits()); reason(t, err) == "" {
			t.Errorf("%s: empty reason", name)
		}
	}
	badCRC := ziptest.Write(ziptest.File("a.txt", []byte("data")))
	central := bytes.Index(badCRC, []byte("PK\x01\x02"))
	binary.LittleEndian.PutUint32(badCRC[central+16:], 0)
	_, err := open(t, badCRC).Read(0)
	if got := reason(t, err); got != "bad member CRC" {
		t.Fatalf("reason = %q", got)
	}
}

func TestRejectsEncryptedAndUnsupportedMethodMembers(t *testing.T) {
	encrypted := ziptest.File("a.txt", nil)
	encrypted.FlagBits = flagEncrypted
	_, err := open(t, ziptest.Write(encrypted)).Read(0)
	if got := reason(t, err); got != "archive member is encrypted" {
		t.Fatalf("reason = %q", got)
	}
	bzip2 := ziptest.File("b.txt", []byte("payload"))
	bzip2.Method = 12
	_, err = open(t, ziptest.Write(bzip2)).Read(0)
	if got := reason(t, err); got != "unsupported archive compression method" {
		t.Fatalf("reason = %q", got)
	}
	for flag, want := range map[uint16]string{
		flagPatched:          "compressed patched data (flag bit 5)",
		flagStrongEncryption: "strong encryption (flag bit 6)",
	} {
		spec := ziptest.File("c.txt", nil)
		spec.FlagBits = flag
		_, err := open(t, ziptest.Write(spec)).Read(0)
		if got := reason(t, err); got != want {
			t.Errorf("flag %#x: reason = %q, want %q", flag, got, want)
		}
	}
}

func zip64Archive(entries, directoryCount uint64) []byte {
	payload := []byte("payload")
	crc := crc32.ChecksumIEEE(payload)
	extra := ziptest.Put(nil, uint16(zip64ExtraID), uint16(24), uint64(7), uint64(7), uint64(0))
	localExtra := ziptest.Put(nil, uint16(zip64ExtraID), uint16(16), uint64(7), uint64(7))
	body := ziptest.Put(nil, uint32(localSignature), uint16(45), uint16(0), uint16(0), uint16(0), uint16(0x21), crc,
		uint32(maxUint32), uint32(maxUint32), uint16(3), uint16(len(localExtra)), []byte("big"), localExtra, payload)
	central := ziptest.Put(nil, uint32(centralSignature), uint16(45), uint16(45), uint16(0), uint16(0), uint16(0),
		uint16(0x21), crc, uint32(maxUint32), uint32(maxUint32), uint16(3), uint16(len(extra)), uint16(0), uint16(0),
		uint16(0), uint32(0), uint32(0), []byte("big"), extra)
	directoryOffset := uint64(len(body))
	out := ziptest.Put(body, central)
	end64Offset := uint64(len(out))
	out = ziptest.Put(out, uint32(end64Signature), uint64(44), uint16(45), uint16(45), uint32(0), uint32(0),
		entries, directoryCount, uint64(len(central)), directoryOffset)
	out = ziptest.Put(out, uint32(end64LocatorSig), uint32(0), end64Offset, uint32(1))
	return ziptest.Put(out, uint32(endSignature), uint16(0), uint16(0), uint16(0xFFFF), uint16(0xFFFF),
		uint32(len(central)), uint32(maxUint32), uint16(0))
}

func TestReadsZIP64SizedMembersViaTheCentralDirectoryExtra(t *testing.T) {
	archive := open(t, zip64Archive(1, 1))
	if len(archive.members) != 1 || archive.members[0].compressedSize != 7 {
		t.Fatalf("members = %+v", archive.members)
	}
	data, err := archive.Read(0)
	if err != nil || string(data) != "payload" {
		t.Fatalf("Read = %q, %v", data, err)
	}
}

func TestDetectsLocalAndCentralNameMismatches(t *testing.T) {
	data := ziptest.Write(ziptest.File("a.txt", []byte("data")))
	data[bytes.Index(data, []byte("PK\x03\x04"))+30] = 'x'
	_, err := open(t, data).Read(0)
	if got := reason(t, err); got != "file name in local header and central directory differ" {
		t.Fatalf("reason = %q", got)
	}
}

func TestRejectsOverlappedEntries(t *testing.T) {
	data := ziptest.Write(ziptest.File("a.txt", []byte("data")), ziptest.File("b.txt", []byte("more")))
	central := bytes.Index(data, []byte("PK\x01\x02"))
	binary.LittleEndian.PutUint32(data[central+20:], 0x7FFF)
	archive := open(t, data)
	_, err := archive.Read(0)
	if got := reason(t, err); got != "overlapped entries" {
		t.Fatalf("reason = %q", got)
	}
	if data, err := archive.Read(1); err != nil || string(data) != "more" {
		t.Fatalf("Read(1) = %q, %v", data, err)
	}
}

func TestRejectsInvalidUTF8NamesOnlyWhenFlagged(t *testing.T) {
	flagged := ziptest.Blank()
	flagged.RawName = []byte{'a', 0xFF}
	flagged.FlagBits = utf8Flag
	_, err := Open(ziptest.Write(flagged), DefaultLimits())
	if got := reason(t, err); got != "archive entry name is not valid UTF-8" {
		t.Fatalf("reason = %q", got)
	}
	unflagged := ziptest.Blank()
	unflagged.RawName = []byte{'a', 0xFF}
	if name := open(t, ziptest.Write(unflagged)).Members()[0].Filename; name != "a " {
		t.Fatalf("name = %q", name)
	}
}

func TestRefusesArchivesOverTheByteAndMemberCaps(t *testing.T) {
	data := ziptest.Write(ziptest.File("a.txt", []byte("data")), ziptest.File("b.txt", []byte("data")))
	limits := DefaultLimits()
	limits.ArchiveBytes = uint64(len(data) - 1)
	_, err := Open(data, limits)
	if got := reason(t, err); got != "archive exceeds the byte limit" {
		t.Fatalf("reason = %q", got)
	}
	limits = DefaultLimits()
	limits.Members = 1
	_, err = Open(data, limits)
	if got := reason(t, err); got != "archive exceeds the member count limit" {
		t.Fatalf("reason = %q", got)
	}
}

func TestRefusesHugeDeclaredMemberCountsBeforeReadingEntries(t *testing.T) {
	_, err := Open(zip64Archive(1<<40, 1<<40), DefaultLimits())
	if got := reason(t, err); got != "archive exceeds the member count limit" {
		t.Fatalf("reason = %q", got)
	}
}

func TestRefusesHugeDeclaredSizesBeforeInflating(t *testing.T) {
	data := ziptest.Write(ziptest.File("a.txt", []byte("data")))
	central := bytes.Index(data, []byte("PK\x01\x02"))
	binary.LittleEndian.PutUint32(data[central+24:], 0xFFFFFFF0)
	_, err := open(t, data).Read(0)
	if got := reason(t, err); got != "archive member exceeds the uncompressed byte limit" {
		t.Fatalf("reason = %q", got)
	}
}

func TestStopsInflatingAtTheDeclaredSize(t *testing.T) {
	bomb := ziptest.File("bomb.bin", make([]byte, 8<<20))
	bomb.Method = 8
	data := ziptest.Write(bomb)
	central := bytes.Index(data, []byte("PK\x01\x02"))
	binary.LittleEndian.PutUint32(data[central+24:], 1024)
	local := bytes.Index(data, []byte("PK\x03\x04"))
	binary.LittleEndian.PutUint32(data[local+22:], 1024)
	archive := open(t, data)
	if archive.members[0].compressedSize > 64<<10 {
		t.Fatalf("bomb did not compress: %d", archive.members[0].compressedSize)
	}
	_, err := archive.Read(0)
	if got := reason(t, err); got != "member size mismatch" {
		t.Fatalf("reason = %q", got)
	}
}

func TestEnforcesTheCumulativeUncompressedCap(t *testing.T) {
	data := ziptest.Write(ziptest.File("a.txt", []byte("1234")), ziptest.File("b.txt", []byte("5678")))
	limits := DefaultLimits()
	limits.TotalBytes = 6
	archive, err := Open(data, limits)
	if err != nil {
		t.Fatal(err)
	}
	if first, err := archive.Read(0); err != nil || string(first) != "1234" {
		t.Fatalf("Read(0) = %q, %v", first, err)
	}
	_, err = archive.Read(1)
	if got := reason(t, err); got != "archive exceeds the total uncompressed byte limit" {
		t.Fatalf("reason = %q", got)
	}
}

func TestRejectsInvalidDeflateData(t *testing.T) {
	spec := ziptest.File("a.txt", []byte("payload"))
	spec.Method = 8
	data := ziptest.Write(spec)
	local := bytes.Index(data, []byte("PK\x03\x04"))
	data[local+30+len("a.txt")] = 0xFF
	_, err := open(t, data).Read(0)
	if got := reason(t, err); got != "member uses invalid deflate data" {
		t.Fatalf("reason = %q", got)
	}
}

func FuzzArchive(f *testing.F) {
	deflated := ziptest.File("d.txt", []byte("hello hello hello"))
	deflated.Method = 8
	f.Add(ziptest.Write(ziptest.Directory("outer"), ziptest.File("outer/a.txt", []byte("markdown"))))
	f.Add(ziptest.Write(deflated))
	f.Add(zip64Archive(1, 1))
	f.Add([]byte("PK\x05\x06"))
	f.Fuzz(func(t *testing.T, data []byte) {
		limits := Limits{ArchiveBytes: 1 << 20, Members: 64, MemberBytes: 1 << 20, TotalBytes: 4 << 20}
		archive, err := Open(data, limits)
		if err != nil {
			return
		}
		for index := range archive.Members() {
			if output, err := archive.Read(index); err == nil && uint64(len(output)) > limits.MemberBytes {
				t.Fatalf("member %d exceeded the cap: %d bytes", index, len(output))
			}
		}
	})
}
