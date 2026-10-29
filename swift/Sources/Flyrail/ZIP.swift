import Compression
import Foundation

private let localSignature: UInt32 = 0x0403_4B50
private let centralSignature: UInt32 = 0x0201_4B50
private let endSignature: UInt32 = 0x0605_4B50
private let end64Signature: UInt32 = 0x0606_4B50
private let locator64Signature: UInt32 = 0x0706_4B50
private let descriptorSignature: UInt32 = 0x0807_4B50
private let zip64ExtraID: UInt16 = 0x0001
private let utf8Flag: UInt16 = 0x0800
private let descriptorFlag: UInt16 = 0x0008
private let encryptedFlag: UInt16 = 0x0001
private let maximumUInt32 = UInt64(UInt32.max)

package struct ZIPMember: Sendable {
    package let name: String
    package let originalName: String
    package let isDirectory: Bool
    package let createSystem: UInt8
    package let externalAttributes: UInt32
}

package struct ZIPArchive: Sendable {
    package let members: [ZIPMember]
    private let archive: [UInt8]
    private let records: [CentralRecord]
    private let boundaries: [UInt64]

    package init(data: Data, limits: BundleLimits) throws {
        guard UInt64(data.count) <= limits.maximumArchiveBytes else {
            throw FlyrailValueError("archive exceeds the byte limit")
        }
        let bytes = Array(data)
        let end = try EndRecord.find(in: bytes)
        let directory: DirectoryRecord = if end.usesZIP64 {
            try DirectoryRecord.zip64(in: bytes, end: end)
        } else {
            DirectoryRecord(
                count: UInt64(end.entryCount),
                size: UInt64(end.directorySize),
                offset: UInt64(end.directoryOffset),
                boundary: end.offset
            )
        }
        try validateZIP64Agreement(end: end, directory: directory)
        guard directory.count <= limits.maximumMemberCount else {
            throw FlyrailValueError("archive exceeds the member count limit")
        }
        let directoryOffset = try checkedIndex(directory.offset, label: "central directory offset")
        let directorySize = try checkedIndex(directory.size, label: "central directory size")
        let directoryEnd = try checkedAdd(directoryOffset, directorySize, label: "central directory range")
        guard directoryEnd == directory.boundary, directoryEnd <= bytes.count else {
            throw FlyrailValueError("central directory range is invalid")
        }
        var cursor = directoryOffset
        var records: [CentralRecord] = []
        try records.reserveCapacity(checkedIndex(directory.count, label: "member count"))
        for _ in 0 ..< directory.count {
            try records.append(CentralRecord.read(bytes, cursor: &cursor, limits: limits))
        }
        guard cursor == directoryEnd else {
            throw FlyrailValueError("central directory size does not match its entries")
        }
        var offsets: Set<UInt64> = []
        for record in records {
            guard record.localOffset < directory.offset else {
                throw FlyrailValueError("local header offset is outside the member area")
            }
            guard offsets.insert(record.localOffset).inserted else {
                throw FlyrailValueError("archive entries share a local header")
            }
        }
        let ordered = records.sorted { $0.localOffset < $1.localOffset }
        var boundaryByOffset: [UInt64: UInt64] = [:]
        boundaryByOffset.reserveCapacity(records.count)
        for (index, record) in ordered.enumerated() {
            let next = index + 1 < ordered.count ? ordered[index + 1].localOffset : directory.offset
            boundaryByOffset[record.localOffset] = next
        }
        members = records.map { record in
            let visibleName = record.name.split(
                separator: "\0",
                maxSplits: 1,
                omittingEmptySubsequences: false
            ).first.map(String.init) ?? ""
            return ZIPMember(
                name: visibleName,
                originalName: record.name,
                isDirectory: visibleName.hasSuffix("/"),
                createSystem: record.createSystem,
                externalAttributes: record.externalAttributes
            )
        }
        archive = bytes
        self.records = records
        boundaries = try records.map { record in
            guard let boundary = boundaryByOffset[record.localOffset] else {
                throw FlyrailValueError("archive member boundary is missing")
            }
            return boundary
        }
    }

    package func data(at index: Int, limits: BundleLimits) throws -> Data {
        guard records.indices.contains(index) else {
            throw FlyrailValueError("archive member index is invalid")
        }
        let record = records[index]
        try validateFlags(record.flags, method: record.method)
        guard record.method == 0 || record.method == 8 else {
            throw FlyrailValueError("unsupported archive compression method")
        }
        guard record.uncompressedSize <= limits.maximumMemberBytes else {
            throw FlyrailValueError("archive member exceeds the uncompressed byte limit")
        }
        guard record.uncompressedSize <= limits.maximumTotalBytes else {
            throw FlyrailValueError("archive exceeds the total uncompressed byte limit")
        }
        return try readMember(
            archive,
            record: record,
            boundary: boundaries[index],
            limits: limits
        )
    }

    package func uncompressedSize(at index: Int) throws -> UInt64 {
        guard records.indices.contains(index) else {
            throw FlyrailValueError("archive member index is invalid")
        }
        return records[index].uncompressedSize
    }
}

private func validateZIP64Agreement(end: EndRecord, directory: DirectoryRecord) throws {
    if end.entryCount != UInt16.max, directory.count != UInt64(end.entryCount) {
        throw FlyrailValueError("ZIP64 member count disagrees with the end record")
    }
    if end.directorySize != UInt32.max, directory.size != UInt64(end.directorySize) {
        throw FlyrailValueError("ZIP64 directory size disagrees with the end record")
    }
    if end.directoryOffset != UInt32.max, directory.offset != UInt64(end.directoryOffset) {
        throw FlyrailValueError("ZIP64 directory offset disagrees with the end record")
    }
}

private struct EndRecord {
    let entryCount: UInt16
    let directorySize: UInt32
    let directoryOffset: UInt32
    let offset: Int

    var usesZIP64: Bool {
        entryCount == UInt16.max ||
            directorySize == UInt32.max ||
            directoryOffset == UInt32.max
    }

    static func find(in data: [UInt8]) throws -> Self {
        guard data.count >= 22 else {
            throw FlyrailValueError("archive is not a ZIP file")
        }
        let minimum = max(0, data.count - 22 - 65535)
        for offset in stride(from: data.count - 22, through: minimum, by: -1) {
            guard try uint32(data, at: offset) == endSignature else {
                continue
            }
            let commentLength = try Int(uint16(data, at: offset + 20))
            guard offset + 22 + commentLength == data.count else {
                continue
            }
            guard try uint16(data, at: offset + 4) == 0,
                  try uint16(data, at: offset + 6) == 0,
                  try uint16(data, at: offset + 8) == uint16(data, at: offset + 10)
            else {
                throw FlyrailValueError("multi-disk ZIP archives are not supported")
            }
            return try Self(
                entryCount: uint16(data, at: offset + 10),
                directorySize: uint32(data, at: offset + 12),
                directoryOffset: uint32(data, at: offset + 16),
                offset: offset
            )
        }
        throw FlyrailValueError("archive is not a ZIP file")
    }
}

private struct DirectoryRecord {
    let count: UInt64
    let size: UInt64
    let offset: UInt64
    let boundary: Int

    static func zip64(in data: [UInt8], end: EndRecord) throws -> Self {
        let locatorOffset = end.offset - 20
        guard locatorOffset >= 0,
              try uint32(data, at: locatorOffset) == locator64Signature
        else {
            throw FlyrailValueError("ZIP64 locator is missing or malformed")
        }
        guard try uint32(data, at: locatorOffset + 4) == 0,
              try uint32(data, at: locatorOffset + 16) == 1
        else {
            throw FlyrailValueError("multi-disk ZIP archives are not supported")
        }
        let recordOffset = try checkedIndex(
            uint64(data, at: locatorOffset + 8),
            label: "ZIP64 end record offset"
        )
        let minimumEnd = try checkedAdd(recordOffset, 56, label: "ZIP64 end record range")
        guard minimumEnd <= locatorOffset,
              try uint32(data, at: recordOffset) == end64Signature
        else {
            throw FlyrailValueError("ZIP64 end record is missing or malformed")
        }
        let bodySize = try checkedIndex(uint64(data, at: recordOffset + 4), label: "ZIP64 end record size")
        let recordSize = try checkedAdd(bodySize, 12, label: "ZIP64 end record size")
        guard bodySize >= 44,
              try checkedAdd(recordOffset, recordSize, label: "ZIP64 end record range") == locatorOffset
        else {
            throw FlyrailValueError("ZIP64 end record size is invalid")
        }
        guard try uint32(data, at: recordOffset + 16) == 0,
              try uint32(data, at: recordOffset + 20) == 0,
              try uint64(data, at: recordOffset + 24) == uint64(data, at: recordOffset + 32)
        else {
            throw FlyrailValueError("multi-disk ZIP archives are not supported")
        }
        return try Self(
            count: uint64(data, at: recordOffset + 32),
            size: uint64(data, at: recordOffset + 40),
            offset: uint64(data, at: recordOffset + 48),
            boundary: recordOffset
        )
    }
}

private struct CentralRecord: Sendable {
    let createSystem: UInt8
    let flags: UInt16
    let method: UInt16
    let checksum: UInt32
    let compressedSize: UInt64
    let uncompressedSize: UInt64
    let externalAttributes: UInt32
    let localOffset: UInt64
    let rawName: [UInt8]
    let name: String
    let usesZIP64Sizes: Bool

    static func read(
        _ data: [UInt8],
        cursor: inout Int,
        limits: BundleLimits
    ) throws -> Self {
        let offset = cursor
        guard try uint32(data, at: offset) == centralSignature else {
            throw FlyrailValueError("central directory entry is malformed")
        }
        guard try uint16(data, at: offset + 34) == 0 else {
            throw FlyrailValueError("multi-disk ZIP archives are not supported")
        }
        let nameLength = try Int(uint16(data, at: offset + 28))
        let extraLength = try Int(uint16(data, at: offset + 30))
        let commentLength = try Int(uint16(data, at: offset + 32))
        guard UInt64(nameLength) <= limits.maximumNameBytes else {
            throw FlyrailValueError("archive member name exceeds the byte limit")
        }
        let body = try checkedAdd(offset, 46, label: "central directory entry")
        let extraStart = try checkedAdd(body, nameLength, label: "central directory name")
        let commentStart = try checkedAdd(extraStart, extraLength, label: "central directory extra")
        cursor = try checkedAdd(commentStart, commentLength, label: "central directory comment")
        guard cursor <= data.count else {
            throw FlyrailValueError("central directory entry is truncated")
        }
        let rawName = Array(data[body ..< extraStart])
        let flags = try uint16(data, at: offset + 8)
        let name = try decodeName(rawName, utf8: flags & utf8Flag != 0)
        let compressed32 = try uint32(data, at: offset + 20)
        let uncompressed32 = try uint32(data, at: offset + 24)
        let local32 = try uint32(data, at: offset + 42)
        let expanded = try expandZIP64(
            compressed: compressed32,
            uncompressed: uncompressed32,
            offset: local32,
            extra: Array(data[extraStart ..< commentStart])
        )
        return try Self(
            createSystem: dataValue(data, at: offset + 5),
            flags: flags,
            method: uint16(data, at: offset + 10),
            checksum: uint32(data, at: offset + 16),
            compressedSize: expanded.compressed,
            uncompressedSize: expanded.uncompressed,
            externalAttributes: uint32(data, at: offset + 38),
            localOffset: expanded.offset,
            rawName: rawName,
            name: name,
            usesZIP64Sizes: compressed32 == UInt32.max || uncompressed32 == UInt32.max
        )
    }
}

private func readMember(
    _ archive: [UInt8],
    record: CentralRecord,
    boundary: UInt64,
    limits: BundleLimits
) throws -> Data {
    let offset = try checkedIndex(record.localOffset, label: "local header offset")
    guard try uint32(archive, at: offset) == localSignature else {
        throw FlyrailValueError("local file header is malformed")
    }
    let flags = try uint16(archive, at: offset + 6)
    let method = try uint16(archive, at: offset + 8)
    guard flags == record.flags, method == record.method else {
        throw FlyrailValueError("local and central archive metadata differ")
    }
    let nameLength = try Int(uint16(archive, at: offset + 26))
    let extraLength = try Int(uint16(archive, at: offset + 28))
    let nameStart = try checkedAdd(offset, 30, label: "local file header")
    let extraStart = try checkedAdd(nameStart, nameLength, label: "local file name")
    let payloadStart = try checkedAdd(extraStart, extraLength, label: "local file extra")
    guard payloadStart <= archive.count else {
        throw FlyrailValueError("local file header is truncated")
    }
    let localName = Array(archive[nameStart ..< extraStart])
    guard localName == record.rawName else {
        throw FlyrailValueError("file name in local header and central directory differ")
    }
    let localCompressed = try uint32(archive, at: offset + 18)
    let localUncompressed = try uint32(archive, at: offset + 22)
    let localChecksum = try uint32(archive, at: offset + 14)
    let localExpanded = try expandZIP64(
        compressed: localCompressed,
        uncompressed: localUncompressed,
        offset: 0,
        extra: Array(archive[extraStart ..< payloadStart]),
        requireOffset: false
    )
    if flags & descriptorFlag == 0 {
        guard localChecksum == record.checksum,
              localExpanded.compressed == record.compressedSize,
              localExpanded.uncompressed == record.uncompressedSize
        else {
            throw FlyrailValueError("local and central archive sizes or CRC differ")
        }
    } else {
        guard localChecksum == 0 || localChecksum == record.checksum,
              localCompressed == 0 || localExpanded.compressed == record.compressedSize,
              localUncompressed == 0 || localExpanded.uncompressed == record.uncompressedSize
        else {
            throw FlyrailValueError("local and central archive sizes or CRC differ")
        }
    }
    let compressedLength = try checkedIndex(record.compressedSize, label: "compressed member size")
    let payloadEnd = try checkedAdd(payloadStart, compressedLength, label: "archive member range")
    let descriptorLength = try dataDescriptorLength(
        archive,
        at: payloadEnd,
        record: record,
        zip64: record.usesZIP64Sizes,
        required: flags & descriptorFlag != 0
    )
    let memberEnd = try checkedAdd(payloadEnd, descriptorLength, label: "archive member range")
    guard UInt64(memberEnd) <= boundary, memberEnd <= archive.count else {
        throw FlyrailValueError("archive entries overlap or are truncated")
    }
    if record.method == 0, record.compressedSize != record.uncompressedSize {
        throw FlyrailValueError("stored archive member sizes differ")
    }
    guard expansionAllowed(
        compressed: record.compressedSize,
        uncompressed: record.uncompressedSize,
        ratio: limits.maximumExpansionRatio
    ) else {
        throw FlyrailValueError("archive member exceeds the expansion ratio limit")
    }
    let payload = Data(archive[payloadStart ..< payloadEnd])
    let output: Data
    switch record.method {
    case 0:
        output = payload
    case 8:
        output = try inflateRawDeflate(payload, expectedSize: record.uncompressedSize)
    default:
        throw FlyrailValueError("unsupported archive compression method")
    }
    guard UInt64(output.count) == record.uncompressedSize else {
        throw FlyrailValueError("archive member size does not match its metadata")
    }
    guard crc32(output) == record.checksum else {
        throw FlyrailValueError("archive member CRC is invalid")
    }
    return output
}

private func dataDescriptorLength(
    _ archive: [UInt8],
    at offset: Int,
    record: CentralRecord,
    zip64: Bool,
    required: Bool
) throws -> Int {
    guard required else {
        return 0
    }
    let unsigned = descriptorLength(
        archive,
        at: offset,
        record: record,
        zip64: zip64,
        signed: false
    )
    if let unsigned {
        return unsigned
    }
    let signed = descriptorLength(
        archive,
        at: offset,
        record: record,
        zip64: zip64,
        signed: true
    )
    guard let signed else {
        throw FlyrailValueError("archive data descriptor differs from the central directory")
    }
    return signed
}

private func descriptorLength(
    _ archive: [UInt8],
    at offset: Int,
    record: CentralRecord,
    zip64: Bool,
    signed: Bool
) -> Int? {
    do {
        var cursor = offset
        if signed {
            guard try uint32(archive, at: cursor) == descriptorSignature else {
                return nil
            }
            cursor += 4
        }
        guard try uint32(archive, at: cursor) == record.checksum else {
            return nil
        }
        cursor += 4
        let compressed: UInt64
        let uncompressed: UInt64
        if zip64 {
            compressed = try uint64(archive, at: cursor)
            uncompressed = try uint64(archive, at: cursor + 8)
            cursor += 16
        } else {
            compressed = try UInt64(uint32(archive, at: cursor))
            uncompressed = try UInt64(uint32(archive, at: cursor + 4))
            cursor += 8
        }
        guard compressed == record.compressedSize, uncompressed == record.uncompressedSize else {
            return nil
        }
        return cursor - offset
    } catch {
        return nil
    }
}

private func validateFlags(_ flags: UInt16, method: UInt16) throws {
    guard flags & encryptedFlag == 0, flags & 0x2060 == 0 else {
        throw FlyrailValueError("encrypted archive members are not supported")
    }
    let compressionOptions: UInt16 = method == 8 ? 0x0006 : 0
    let allowed = utf8Flag | descriptorFlag | compressionOptions
    guard flags & ~allowed == 0 else {
        throw FlyrailValueError("archive member uses unsupported flags")
    }
}

private func expandZIP64(
    compressed: UInt32,
    uncompressed: UInt32,
    offset: UInt32,
    extra: [UInt8],
    requireOffset: Bool = true
) throws -> (compressed: UInt64, uncompressed: UInt64, offset: UInt64) {
    var compressedValue = UInt64(compressed)
    var uncompressedValue = UInt64(uncompressed)
    var offsetValue = UInt64(offset)
    let needsCompressed = compressed == UInt32.max
    let needsUncompressed = uncompressed == UInt32.max
    let needsOffset = requireOffset && offset == UInt32.max
    var cursor = 0
    var found = false
    while cursor < extra.count {
        guard cursor + 4 <= extra.count else {
            throw FlyrailValueError("archive extra field is truncated")
        }
        let identifier = try uint16(extra, at: cursor)
        let size = try Int(uint16(extra, at: cursor + 2))
        let body = cursor + 4
        let end = try checkedAdd(body, size, label: "archive extra field")
        guard end <= extra.count else {
            throw FlyrailValueError("archive extra field is truncated")
        }
        if identifier == zip64ExtraID {
            guard !found else {
                throw FlyrailValueError("archive has duplicate ZIP64 extra fields")
            }
            found = true
            var field = body
            if needsUncompressed {
                guard field + 8 <= end else {
                    throw FlyrailValueError("ZIP64 extra field is truncated")
                }
                uncompressedValue = try uint64(extra, at: field)
                field += 8
            }
            if needsCompressed {
                guard field + 8 <= end else {
                    throw FlyrailValueError("ZIP64 extra field is truncated")
                }
                compressedValue = try uint64(extra, at: field)
                field += 8
            }
            if needsOffset {
                guard field + 8 <= end else {
                    throw FlyrailValueError("ZIP64 extra field is truncated")
                }
                offsetValue = try uint64(extra, at: field)
            }
        }
        cursor = end
    }
    guard !(needsCompressed || needsUncompressed || needsOffset) || found else {
        throw FlyrailValueError("ZIP64 extra field is missing")
    }
    return (compressedValue, uncompressedValue, offsetValue)
}

private func expansionAllowed(compressed: UInt64, uncompressed: UInt64, ratio: UInt64) -> Bool {
    if uncompressed == 0 {
        return true
    }
    guard compressed > 0 else {
        return false
    }
    let product = compressed.multipliedReportingOverflow(by: ratio)
    return product.overflow || uncompressed <= product.partialValue
}

private func inflateRawDeflate(_ payload: Data, expectedSize: UInt64) throws -> Data {
    let expected = try checkedIndex(expectedSize, label: "uncompressed member size")
    let dummy = UnsafeMutablePointer<UInt8>.allocate(capacity: 1)
    defer { dummy.deallocate() }
    var stream = compression_stream(
        dst_ptr: dummy,
        dst_size: 0,
        src_ptr: UnsafePointer(dummy),
        src_size: 0,
        state: nil
    )
    guard compression_stream_init(&stream, COMPRESSION_STREAM_DECODE, COMPRESSION_ZLIB) ==
        COMPRESSION_STATUS_OK
    else {
        throw FlyrailValueError("cannot initialize DEFLATE decoder")
    }
    defer { compression_stream_destroy(&stream) }
    var output = Data()
    output.reserveCapacity(expected)
    let chunkSize = min(65536, max(1, expected))
    var chunk = [UInt8](repeating: 0, count: chunkSize)
    let status = payload.withUnsafeBytes { source -> compression_status in
        stream.src_ptr = source.bindMemory(to: UInt8.self).baseAddress ?? UnsafePointer(dummy)
        stream.src_size = source.count
        while true {
            let result = chunk.withUnsafeMutableBytes { destination -> compression_status in
                stream.dst_ptr = destination.bindMemory(to: UInt8.self).baseAddress ?? dummy
                stream.dst_size = destination.count
                return compression_stream_process(&stream, Int32(COMPRESSION_STREAM_FINALIZE.rawValue))
            }
            let produced = chunk.count - stream.dst_size
            if produced > 0 {
                output.append(contentsOf: chunk[0 ..< produced])
                if output.count > expected {
                    return COMPRESSION_STATUS_ERROR
                }
            }
            if result != COMPRESSION_STATUS_OK {
                return result
            }
            if produced == 0, stream.src_size == 0 {
                return COMPRESSION_STATUS_ERROR
            }
        }
    }
    guard status == COMPRESSION_STATUS_END, stream.src_size == 0, output.count == expected else {
        throw FlyrailValueError("archive member uses invalid DEFLATE data")
    }
    return output
}

private func decodeName(_ bytes: [UInt8], utf8: Bool) throws -> String {
    if utf8 {
        guard let value = String(bytes: bytes, encoding: .utf8) else {
            throw FlyrailValueError("archive entry name is not valid UTF-8")
        }
        return value
    }
    return String(String.UnicodeScalarView(bytes.compactMap { Unicode.Scalar(cp437[Int($0)]) }))
}

private let cp437: [UInt32] = Array(0 ... 127).map(UInt32.init) + [
    0x00C7, 0x00FC, 0x00E9, 0x00E2, 0x00E4, 0x00E0, 0x00E5, 0x00E7,
    0x00EA, 0x00EB, 0x00E8, 0x00EF, 0x00EE, 0x00EC, 0x00C4, 0x00C5,
    0x00C9, 0x00E6, 0x00C6, 0x00F4, 0x00F6, 0x00F2, 0x00FB, 0x00F9,
    0x00FF, 0x00D6, 0x00DC, 0x00A2, 0x00A3, 0x00A5, 0x20A7, 0x0192,
    0x00E1, 0x00ED, 0x00F3, 0x00FA, 0x00F1, 0x00D1, 0x00AA, 0x00BA,
    0x00BF, 0x2310, 0x00AC, 0x00BD, 0x00BC, 0x00A1, 0x00AB, 0x00BB,
    0x2591, 0x2592, 0x2593, 0x2502, 0x2524, 0x2561, 0x2562, 0x2556,
    0x2555, 0x2563, 0x2551, 0x2557, 0x255D, 0x255C, 0x255B, 0x2510,
    0x2514, 0x2534, 0x252C, 0x251C, 0x2500, 0x253C, 0x255E, 0x255F,
    0x255A, 0x2554, 0x2569, 0x2566, 0x2560, 0x2550, 0x256C, 0x2567,
    0x2568, 0x2564, 0x2565, 0x2559, 0x2558, 0x2552, 0x2553, 0x256B,
    0x256A, 0x2518, 0x250C, 0x2588, 0x2584, 0x258C, 0x2590, 0x2580,
    0x03B1, 0x00DF, 0x0393, 0x03C0, 0x03A3, 0x03C3, 0x00B5, 0x03C4,
    0x03A6, 0x0398, 0x03A9, 0x03B4, 0x221E, 0x03C6, 0x03B5, 0x2229,
    0x2261, 0x00B1, 0x2265, 0x2264, 0x2320, 0x2321, 0x00F7, 0x2248,
    0x00B0, 0x2219, 0x00B7, 0x221A, 0x207F, 0x00B2, 0x25A0, 0x00A0,
]

private func crc32(_ data: Data) -> UInt32 {
    var checksum = UInt32.max
    for byte in data {
        checksum ^= UInt32(byte)
        for _ in 0 ..< 8 {
            checksum = checksum & 1 == 0 ? checksum >> 1 : (checksum >> 1) ^ 0xEDB8_8320
        }
    }
    return ~checksum
}

private func checkedIndex(_ value: UInt64, label: String) throws -> Int {
    guard let result = Int(exactly: value) else {
        throw FlyrailValueError("\(label) exceeds the platform index range")
    }
    return result
}

private func checkedAdd(_ left: Int, _ right: Int, label: String) throws -> Int {
    let result = left.addingReportingOverflow(right)
    guard !result.overflow else {
        throw FlyrailValueError("\(label) overflows the platform index range")
    }
    return result.partialValue
}

private func checkedAdd(_ left: UInt64, _ right: UInt64, label: String) throws -> UInt64 {
    let result = left.addingReportingOverflow(right)
    guard !result.overflow else {
        throw FlyrailValueError("\(label) overflows UInt64")
    }
    return result.partialValue
}

private func dataValue(_ data: [UInt8], at offset: Int) throws -> UInt8 {
    guard data.indices.contains(offset) else {
        throw FlyrailValueError("archive field is truncated")
    }
    return data[offset]
}

private func uint16(_ data: [UInt8], at offset: Int) throws -> UInt16 {
    guard offset >= 0, offset + 2 <= data.count else {
        throw FlyrailValueError("archive field is truncated")
    }
    return UInt16(data[offset]) | UInt16(data[offset + 1]) << 8
}

private func uint32(_ data: [UInt8], at offset: Int) throws -> UInt32 {
    guard offset >= 0, offset + 4 <= data.count else {
        throw FlyrailValueError("archive field is truncated")
    }
    return UInt32(data[offset]) |
        UInt32(data[offset + 1]) << 8 |
        UInt32(data[offset + 2]) << 16 |
        UInt32(data[offset + 3]) << 24
}

private func uint64(_ data: [UInt8], at offset: Int) throws -> UInt64 {
    guard offset >= 0, offset + 8 <= data.count else {
        throw FlyrailValueError("archive field is truncated")
    }
    return (0 ..< 8).reduce(0) { result, index in
        result | UInt64(data[offset + index]) << UInt64(index * 8)
    }
}
