package zipx

import (
	"bytes"
	"compress/flate"
	"encoding/binary"
	"hash/crc32"
	"io"
	"slices"
	"strings"
	"unicode/utf8"
)

const (
	localSignature       = 0x04034B50
	centralSignature     = 0x02014B50
	endSignature         = 0x06054B50
	end64Signature       = 0x06064B50
	end64LocatorSig      = 0x07064B50
	zip64ExtraID         = 0x0001
	utf8Flag             = 0x0800
	maxUint32            = 0xFFFFFFFF
	localHeaderSize      = 30
	centralHeaderSize    = 46
	endRecordSize        = 22
	end64RecordSize      = 56
	end64LocatorSize     = 20
	maxEndCommentLength  = 65_535
	methodStored         = 0
	methodDeflated       = 8
	flagEncrypted        = 0x0001
	flagPatched          = 0x0020
	flagStrongEncryption = 0x0040
)

// The default caps on untrusted archives.
const (
	MaxArchiveBytes = 256 << 20
	MaxMembers      = 100_000
	MaxMemberBytes  = 64 << 20
	MaxTotalBytes   = 512 << 20
)

// Limits caps the archive size, member count, and per-member and cumulative uncompressed bytes.
type Limits struct {
	ArchiveBytes uint64
	Members      uint64
	MemberBytes  uint64
	TotalBytes   uint64
}

// DefaultLimits returns the caps Flyrail applies to bundle archives.
func DefaultLimits() Limits {
	return Limits{ArchiveBytes: MaxArchiveBytes, Members: MaxMembers, MemberBytes: MaxMemberBytes, TotalBytes: MaxTotalBytes}
}

// Error is a malformed, unsupported or oversized archive.
type Error struct {
	Reason string
}

// Error returns the reason.
func (e *Error) Error() string {
	return e.Reason
}

func bad(reason string) *Error {
	return &Error{Reason: reason}
}

// ArchiveTooLarge returns the error for an archive over its byte cap.
func ArchiveTooLarge() *Error {
	return bad("archive exceeds the byte limit")
}

// Member is one central directory entry.
type Member struct {
	Filename     string
	OrigFilename string
	IsDir        bool
	CreateSystem uint8
	ExternalAttr uint32

	method           uint16
	flags            uint16
	crc32            uint32
	offset           uint64
	endOffset        uint64
	compressedSize   uint64
	uncompressedSize uint64
}

// Archive is a parsed ZIP archive held in memory; it is not safe for concurrent use.
type Archive struct {
	data    []byte
	members []Member
	limits  Limits
	total   uint64
}

// Open parses the end records and central directory of data.
func Open(data []byte, limits Limits) (*Archive, error) {
	if uint64(len(data)) > limits.ArchiveBytes {
		return nil, ArchiveTooLarge()
	}
	end, err := findEnd(data)
	if err != nil {
		return nil, err
	}
	count, offset := uint64(end.count), end.offset
	if end.zip64 {
		at, err := findLocator(data, end.at)
		if err != nil {
			return nil, err
		}
		count, offset, err = readEnd64(data, at)
		if err != nil {
			return nil, err
		}
	}
	if count > limits.Members {
		return nil, bad("archive exceeds the member count limit")
	}
	var members []Member
	cursor := offset
	for range count {
		member, err := readCentral(data, &cursor)
		if err != nil {
			return nil, err
		}
		members = append(members, member)
	}
	order := make([]int, len(members))
	for index := range order {
		order[index] = index
	}
	slices.SortStableFunc(order, func(a, b int) int {
		switch {
		case members[a].offset > members[b].offset:
			return -1
		case members[a].offset < members[b].offset:
			return 1
		}
		return 0
	})
	endOffset := offset
	for _, index := range order {
		members[index].endOffset = endOffset
		endOffset = members[index].offset
	}
	return &Archive{data: data, members: members, limits: limits}, nil
}

// Members returns the central directory entries in archive order.
func (a *Archive) Members() []Member {
	return slices.Clone(a.members)
}

// Read returns the verified bytes of the member at index.
func (a *Archive) Read(index int) ([]byte, error) {
	if index < 0 || index >= len(a.members) {
		return nil, bad("archive member index is invalid")
	}
	member := &a.members[index]
	at := member.offset
	signature, err := u32At(a.data, at)
	if err != nil {
		return nil, err
	}
	if signature != localSignature {
		return nil, bad("bad local file header magic")
	}
	if member.flags&flagPatched != 0 {
		return nil, bad("compressed patched data (flag bit 5)")
	}
	if member.flags&flagStrongEncryption != 0 {
		return nil, bad("strong encryption (flag bit 6)")
	}
	flags, err := u16At(a.data, at+6)
	if err != nil {
		return nil, err
	}
	nameLength, err := u16At(a.data, at+26)
	if err != nil {
		return nil, err
	}
	extraLength, err := u16At(a.data, at+28)
	if err != nil {
		return nil, err
	}
	name, ok := slice(a.data, at+localHeaderSize, uint64(nameLength))
	if !ok {
		return nil, bad("truncated local file name")
	}
	decoded, err := decodeName(name, flags&utf8Flag != 0)
	if err != nil {
		return nil, err
	}
	if decoded != member.OrigFilename {
		return nil, bad("file name in local header and central directory differ")
	}
	start := at + localHeaderSize + uint64(nameLength) + uint64(extraLength)
	if member.endOffset != member.offset {
		if end := start + member.compressedSize; end >= start && end > member.endOffset {
			return nil, bad("overlapped entries")
		}
	}
	if member.flags&flagEncrypted != 0 {
		return nil, bad("archive member is encrypted")
	}
	payload, ok := slice(a.data, start, member.compressedSize)
	if !ok {
		return nil, bad("truncated member")
	}
	size := member.uncompressedSize
	if size > a.limits.MemberBytes {
		return nil, bad("archive member exceeds the uncompressed byte limit")
	}
	if size > a.limits.TotalBytes || a.total > a.limits.TotalBytes-size {
		return nil, bad("archive exceeds the total uncompressed byte limit")
	}
	output, err := inflate(member.method, payload, size)
	if err != nil {
		return nil, err
	}
	if uint64(len(output)) != size {
		return nil, bad("member size mismatch")
	}
	if crc32.ChecksumIEEE(output) != member.crc32 {
		return nil, bad("bad member CRC")
	}
	a.total += size
	return output, nil
}

func inflate(method uint16, payload []byte, size uint64) ([]byte, error) {
	switch method {
	case methodStored:
		if uint64(len(payload)) != size {
			return nil, bad("member size mismatch")
		}
		return bytes.Clone(payload), nil
	case methodDeflated:
		reader := flate.NewReader(bytes.NewReader(payload))
		output, err := io.ReadAll(io.LimitReader(reader, int64(size)+1)) //nolint:gosec // size is capped at MemberBytes
		closeErr := reader.Close()
		if err != nil || closeErr != nil {
			return nil, bad("member uses invalid deflate data")
		}
		return output, nil
	}
	return nil, bad("unsupported archive compression method")
}

type endRecord struct {
	count  uint16
	offset uint64
	at     uint64
	zip64  bool
}

func findEnd(data []byte) (endRecord, error) {
	length := uint64(len(data))
	if length < endRecordSize {
		return endRecord{}, bad("archive is not a zip file")
	}
	minimum := uint64(0)
	if length > maxEndCommentLength+endRecordSize {
		minimum = length - maxEndCommentLength - endRecordSize
	}
	for at := length - endRecordSize; ; at-- {
		signature, err := u32At(data, at)
		if err != nil {
			return endRecord{}, err
		}
		comment, err := u16At(data, at+20)
		if err != nil {
			return endRecord{}, err
		}
		if signature == endSignature && uint64(comment) == length-at-endRecordSize {
			count, err := u16At(data, at+10)
			if err != nil {
				return endRecord{}, err
			}
			offset, err := u32At(data, at+16)
			if err != nil {
				return endRecord{}, err
			}
			return endRecord{
				count:  count,
				offset: uint64(offset),
				at:     at,
				zip64:  count == 0xFFFF || offset == maxUint32,
			}, nil
		}
		if at == minimum {
			return endRecord{}, bad("archive is not a zip file")
		}
	}
}

func findLocator(data []byte, endAt uint64) (uint64, error) {
	if endAt < end64LocatorSize {
		return 0, bad("truncated zip64 locator")
	}
	at := endAt - end64LocatorSize
	signature, err := u32At(data, at)
	if err != nil {
		return 0, err
	}
	if signature != end64LocatorSig {
		return 0, bad("bad zip64 locator magic")
	}
	return u64At(data, at+8)
}

func readEnd64(data []byte, at uint64) (count, offset uint64, err error) {
	if _, ok := slice(data, at, end64RecordSize); !ok {
		return 0, 0, bad("truncated zip64 end record")
	}
	signature, err := u32At(data, at)
	if err != nil {
		return 0, 0, err
	}
	if signature != end64Signature {
		return 0, 0, bad("bad zip64 end record magic")
	}
	if count, err = u64At(data, at+32); err != nil {
		return 0, 0, err
	}
	if offset, err = u64At(data, at+48); err != nil {
		return 0, 0, err
	}
	return count, offset, nil
}

func readCentral(data []byte, cursor *uint64) (Member, error) {
	at := *cursor
	header, ok := slice(data, at, centralHeaderSize)
	if !ok {
		return Member{}, bad("truncated central directory entry")
	}
	if binary.LittleEndian.Uint32(header) != centralSignature {
		return Member{}, bad("bad central directory magic")
	}
	flags := binary.LittleEndian.Uint16(header[8:])
	nameLength := uint64(binary.LittleEndian.Uint16(header[28:]))
	extraLength := uint64(binary.LittleEndian.Uint16(header[30:]))
	commentLength := uint64(binary.LittleEndian.Uint16(header[32:]))
	body := at + centralHeaderSize
	name, ok := slice(data, body, nameLength)
	if !ok {
		return Member{}, bad("truncated central directory entry")
	}
	extra, ok := slice(data, body+nameLength, extraLength)
	if !ok {
		return Member{}, bad("truncated central directory entry")
	}
	compressed, uncompressed, offset := expandZIP64(
		binary.LittleEndian.Uint32(header[20:]),
		binary.LittleEndian.Uint32(header[24:]),
		binary.LittleEndian.Uint32(header[42:]),
		extra,
	)
	*cursor = body + nameLength + extraLength + commentLength
	if *cursor > uint64(len(data)) {
		return Member{}, bad("truncated central directory entry")
	}
	original, err := decodeName(name, flags&utf8Flag != 0)
	if err != nil {
		return Member{}, err
	}
	filename, _, _ := strings.Cut(original, "\x00")
	return Member{
		Filename:         filename,
		OrigFilename:     original,
		IsDir:            strings.HasSuffix(filename, "/"),
		CreateSystem:     header[5],
		ExternalAttr:     binary.LittleEndian.Uint32(header[38:]),
		method:           binary.LittleEndian.Uint16(header[10:]),
		flags:            flags,
		crc32:            binary.LittleEndian.Uint32(header[16:]),
		offset:           offset,
		compressedSize:   compressed,
		uncompressedSize: uncompressed,
	}, nil
}

func decodeName(raw []byte, isUTF8 bool) (string, error) {
	if isUTF8 {
		if !utf8.Valid(raw) {
			return "", bad("archive entry name is not valid UTF-8")
		}
		return string(raw), nil
	}
	var out strings.Builder
	for _, b := range raw {
		if b < 0x80 {
			out.WriteByte(b)
		} else {
			out.WriteRune(cp437High[b-0x80])
		}
	}
	return out.String(), nil
}

func expandZIP64(compressed32, uncompressed32, offset32 uint32, extra []byte) (compressed, uncompressed, offset uint64) {
	compressed, uncompressed, offset = uint64(compressed32), uint64(uncompressed32), uint64(offset32)
	var cursor uint64
	for {
		id, idErr := u16At(extra, cursor)
		size, sizeErr := u16At(extra, cursor+2)
		if idErr != nil || sizeErr != nil {
			break
		}
		body := cursor + 4
		end := body + uint64(size)
		cursor = end
		if id != zip64ExtraID {
			continue
		}
		field := body
		for _, value := range []*uint64{&uncompressed, &compressed, &offset} {
			if *value == maxUint32 && field+8 <= end {
				if replacement, err := u64At(extra, field); err == nil {
					*value = replacement
				}
				field += 8
			}
		}
		break
	}
	return compressed, uncompressed, offset
}

func slice(data []byte, at, length uint64) ([]byte, bool) {
	end := at + length
	if end < at || end > uint64(len(data)) {
		return nil, false
	}
	return data[at:end], true
}

func u16At(data []byte, at uint64) (uint16, error) {
	field, ok := slice(data, at, 2)
	if !ok {
		return 0, bad("truncated field")
	}
	return binary.LittleEndian.Uint16(field), nil
}

func u32At(data []byte, at uint64) (uint32, error) {
	field, ok := slice(data, at, 4)
	if !ok {
		return 0, bad("truncated field")
	}
	return binary.LittleEndian.Uint32(field), nil
}

func u64At(data []byte, at uint64) (uint64, error) {
	field, ok := slice(data, at, 8)
	if !ok {
		return 0, bad("truncated field")
	}
	return binary.LittleEndian.Uint64(field), nil
}
