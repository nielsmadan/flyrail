import Flyrail
import Foundation

package func runInterop(
    arguments: [String], standardInput: FileHandle, standardOutput: FileHandle, standardError: FileHandle,
) -> Int32 {
    guard let name = arguments.first, let command = InteropCommand(rawValue: name) else {
        let commands = "differential|render-hooks|encode|decode|write|install|update|inspect|remove|recover|records|hold|observe"
        try? standardError.write(contentsOf: Data("usage: FlyrailInterop \(commands) [options]\n".utf8))
        return 1
    }
    if [.hold, .write, .install, .update, .inspect, .remove, .recover].contains(command) ||
        (command == .records && arguments.count > 1)
    {
        do {
            try NativeInterop.run(command, arguments: Array(arguments.dropFirst()), output: standardOutput)
            return 0
        } catch {
            let message = (error as? TargetError)?.message ?? "\(error)"
            let value = JSONValue.object([("error", .object([
                ("code", .null), ("message", .string(message)), ("path", .null),
            ]))])
            try? standardOutput.write(contentsOf: JSONWriter.canonicalData(value) + Data([10]))
            return 1
        }
    }
    do {
        if command == .differential {
            try standardOutput.write(contentsOf: InteropDifferential.run(Array(arguments.dropFirst())))
            return 0
        }
        if command == .renderHooks {
            guard arguments.count == 1 else { throw FlyrailValueError("unexpected hook rendering arguments") }
            try standardOutput.write(contentsOf: InteropHookRendering.run(boundedStateInput(standardInput)))
            return 0
        }
        if command == .observe {
            try NativeInterop.run(command, arguments: Array(arguments.dropFirst()), output: standardOutput)
            return 0
        }
        guard command == .encode || command == .decode || command == .records else {
            throw FlyrailValueError("command requires the native resource layer")
        }
        let expected: String?
        switch Array(arguments.dropFirst()) {
        case []: expected = nil
        case let options where options.count == 2 && options[0] == "--type" && command != .records:
            expected = options[1]
        default: throw FlyrailValueError("unexpected interop arguments")
        }
        let input = try boundedStateInput(standardInput)
        if command == .records {
            try standardOutput.write(contentsOf: interopRecords(input))
        } else {
            let output = try StateCodec.transcode(input)
            if let expected {
                guard try stateRecordTag(JSONParser.parse(output)) == expected else {
                    throw FlyrailValueError("metadata record has the wrong type")
                }
            }
            try standardOutput.write(contentsOf: output)
        }
        return 0
    } catch let error as TargetError {
        try? standardError.write(contentsOf: Data("\(error.message)\n".utf8))
        return 1
    } catch {
        try? standardError.write(contentsOf: Data("\(error)\n".utf8))
        return 1
    }
}

private func boundedStateInput(_ input: FileHandle) throws -> Data {
    try boundedInteropInput(input, limit: JSONLimits.default.maximumInputBytes,
                            message: "JSON input exceeds the byte limit")
}

func boundedInteropInput(_ input: FileHandle, limit: Int, message: String) throws -> Data {
    var data = Data()
    while let chunk = try input.read(upToCount: min(65536, limit + 1 - data.count)), !chunk.isEmpty {
        data.append(chunk)
        guard data.count <= limit else { throw FlyrailValueError(message) }
    }
    return data
}

private func interopRecords(_ input: Data) throws -> Data {
    let request = try JSONParser.parse(input)
    guard case let .array(records) = request else {
        throw FlyrailValueError("records expects an array of path/bytes records on standard input")
    }
    let results = try records.map { item -> JSONValue in
        guard case let .object(fields) = item, fields.count == 2,
              case let .string(path) = fields.first(where: { $0.0 == "path" })?.1,
              case let .string(encoded) = fields.first(where: { $0.0 == "bytes" })?.1,
              let raw = Data(base64Encoded: encoded), raw.base64EncodedString() == encoded
        else { throw FlyrailValueError("invalid interop record") }
        _ = try StateCodec.decodePath(.object([("path", .string(path))]))
        let canonical = try StateCodec.transcode(raw)
        guard let tag = try stateRecordTag(JSONParser.parse(canonical)) else {
            throw FlyrailValueError("record type missing")
        }
        return .object([
            ("path", .string(path)), ("type", .string(tag)), ("bytes", .string(encoded)),
            ("canonical", .string(canonical.base64EncodedString())),
        ])
    }
    return try StateCodec.encodeRecord(.object([("records", .array(results))]))
}
