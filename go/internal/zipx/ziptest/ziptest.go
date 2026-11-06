// Package ziptest writes small ZIP archives with chosen metadata for tests.
package ziptest

import (
	"bytes"
	"compress/flate"
	"encoding/binary"
	"hash/crc32"
	"strings"
)

const (
	versionNeeded = 20
	utf8Flag      = 0x0800
)

// Spec describes one archive member.
type Spec struct {
	Name         string
	RawName      []byte
	Data         []byte
	Method       uint16
	FlagBits     uint16
	CreateSystem uint8
	ExternalAttr uint32
}

// Blank returns an unnamed, empty, stored member made on a Unix system.
func Blank() Spec {
	return Spec{CreateSystem: 3}
}

// File returns a stored file member.
func File(name string, data []byte) Spec {
	spec := Blank()
	spec.Name = name
	spec.Data = data
	return spec
}

// Directory returns a directory member, adding the trailing slash when name lacks one.
func Directory(name string) Spec {
	if !strings.HasSuffix(name, "/") {
		name += "/"
	}
	return File(name, nil)
}

func (s Spec) nameBytes() []byte {
	if s.RawName != nil {
		return s.RawName
	}
	return []byte(s.Name)
}

func (s Spec) flags() uint16 {
	if s.RawName == nil && strings.ContainsFunc(s.Name, func(r rune) bool { return r >= 0x80 }) {
		return s.FlagBits | utf8Flag
	}
	return s.FlagBits
}

func deflate(data []byte) []byte {
	var out bytes.Buffer
	writer, _ := flate.NewWriter(&out, 6)
	_, _ = writer.Write(data)
	_ = writer.Close()
	return out.Bytes()
}

// Put appends little-endian encodings of uint16, uint32, uint64 and []byte values to out.
func Put(out []byte, values ...any) []byte {
	for _, value := range values {
		switch v := value.(type) {
		case uint16:
			out = binary.LittleEndian.AppendUint16(out, v)
		case uint32:
			out = binary.LittleEndian.AppendUint32(out, v)
		case uint64:
			out = binary.LittleEndian.AppendUint64(out, v)
		case []byte:
			out = append(out, v...)
		}
	}
	return out
}

// Write returns an archive holding specs in order.
func Write(specs ...Spec) []byte {
	var body, central []byte
	for _, spec := range specs {
		payload := spec.Data
		if spec.Method == 8 {
			payload = deflate(spec.Data)
		}
		crc := crc32.ChecksumIEEE(spec.Data)
		offset := uint32(len(body))
		name := spec.nameBytes()
		stored, size, nameLength := uint32(len(payload)), uint32(len(spec.Data)), uint16(len(name))
		body = Put(body, uint32(0x04034B50), uint16(versionNeeded), spec.flags(), spec.Method, uint16(0), uint16(0x21),
			crc, stored, size, nameLength, uint16(0), name, payload)
		central = Put(central, uint32(0x02014B50), uint16(spec.CreateSystem)<<8, uint16(versionNeeded), spec.flags(),
			spec.Method, uint16(0), uint16(0x21), crc, stored, size, nameLength, uint16(0), uint16(0), uint16(0),
			uint16(0), spec.ExternalAttr, offset, name)
	}
	count := uint16(len(specs))
	out := Put(body, central)
	return Put(out, uint32(0x06054B50), uint16(0), uint16(0), count, count, uint32(len(central)), uint32(len(body)), uint16(0))
}
