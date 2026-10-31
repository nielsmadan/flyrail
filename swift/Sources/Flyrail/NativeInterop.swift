import Darwin
import Foundation

package enum InteropCommand: String, Sendable {
    case differential
    case encode
    case decode
    case write
    case install
    case update
    case inspect
    case remove
    case recover
    case records
    case hold
    case observe
    case renderHooks = "render-hooks"
}

package enum NativeInterop {
    package static func run(_ command: InteropCommand, arguments: [String], output: FileHandle) throws {
        let allowed: Set<String>
        switch command {
        case .hold: allowed = ["--state", "--destination", "--kind", "--barrier", "--lock-timeout", "--wait-timeout"]
        case .observe: allowed = ["--destination", "--kind", "--repeat"]
        case .records: allowed = ["--state"]
        case .write, .install, .update:
            allowed = ["--state", "--bundle-id", "--lock-timeout", "--destination", "--version",
                       "--payload", "--large", "--mode", "--acquisition", "--replace-modified",
                       "--fail-index-completion", "--barrier-phase", "--barrier-ready", "--barrier-release",
                       "--barrier-timeout", "--fault-phase", "--kind", "--section", "--source", "--agent", "--node"]
        case .inspect: allowed = ["--state", "--bundle-id", "--lock-timeout"]
        case .recover:
            allowed = ["--state", "--bundle-id", "--lock-timeout", "--barrier-phase", "--barrier-ready",
                       "--barrier-release", "--barrier-timeout", "--fault-phase"]
        case .remove: allowed = ["--state", "--bundle-id", "--lock-timeout", "--replace-modified"]
        default: throw FlyrailValueError("unsupported native interop command")
        }
        let options = try options(arguments, allowed: allowed)
        switch command {
        case .hold: try hold(options, output: output)
        case .observe:
            guard let count = Int(options["--repeat"] ?? "1"), (1 ... 1000).contains(count) else {
                throw FlyrailValueError("repeat must be between 1 and 1000")
            }
            let descriptors = count > 1 ? openDescriptors() : nil
            for _ in 0 ..< count {
                try observe(options, output: output)
                if let descriptors, openDescriptors() != descriptors {
                    throw FlyrailValueError("observation retained filesystem descriptors")
                }
            }
        case .records: try records(options, output: output)
        case .write, .install, .update, .inspect, .remove, .recover:
            try operate(command, options: options, output: output)
        default: throw FlyrailValueError("unsupported native interop command")
        }
    }

    private static func openDescriptors() -> [Int32] {
        (0 ..< getdtablesize()).filter { fcntl($0, F_GETFD) >= 0 }
    }

    private static func options(_ arguments: [String], allowed: Set<String>) throws -> [String: String] {
        var options: [String: String] = [:]
        var index = 0
        while index < arguments.count {
            let name = arguments[index]
            guard allowed.contains(name), options[name] == nil else {
                throw FlyrailValueError("unexpected or repeated interop argument: \(name)")
            }
            if ["--replace-modified", "--fail-index-completion"].contains(name) {
                options[name] = "true"
                index += 1
            } else {
                guard index + 1 < arguments.count, !arguments[index + 1].hasPrefix("--") else {
                    throw FlyrailValueError("interop option requires a value")
                }
                options[name] = arguments[index + 1]
                index += 2
            }
        }
        return options
    }

    private static func required(_ options: [String: String], _ key: String) throws -> String {
        guard let value = options[key] else { throw FlyrailValueError("missing required argument: \(key)") }
        return value
    }

    private static func resource(_ options: [String: String]) throws -> ResourceRef {
        guard let kind = ResourceKind(rawValue: options["--kind"] ?? "document") else {
            throw FlyrailValueError("invalid resource kind")
        }
        return try NativeAuthority.reference(ResourcePath(required(options, "--destination")), kind: kind)
    }

    package static func duration(_ name: String, _ value: String) throws -> Duration {
        guard let seconds = Double(value) else {
            throw FlyrailValueError(
                "invalid \(name): " +
                    (value.isEmpty ? "cannot parse float from empty string" : "invalid float literal"),
            )
        }
        let reason = "invalid \(name): cannot convert float seconds to Duration: value is"
        guard !(seconds < 0) else { throw FlyrailValueError("\(reason) negative") }
        guard seconds < 18_446_744_073_709_551_616 else { throw FlyrailValueError("\(reason) either too big or NaN") }
        return seconds >= maximumSeconds ? .seconds(maximumSeconds) : .seconds(seconds)
    }

    package static let maximumSeconds: Double = 4e18

    private static func emit(_ value: JSONValue, output: FileHandle) throws {
        try output.write(contentsOf: StateCodec.encodeRecord(value))
    }

    private static func hold(_ options: [String: String], output: FileHandle) throws {
        let store = try DirectoryStore(ResourcePath(required(options, "--state")))
        let reference = try resource(options)
        let barrier = try NativeAuthority.absolute(ResourcePath(required(options, "--barrier")))
        let wait = try duration("--wait-timeout", options["--wait-timeout"] ?? "30")
        let timeout = try duration("--lock-timeout", options["--lock-timeout"] ?? "0")
        let lock = try StateRecords.lock(store, name: StateKey.resourceLock(reference.destination), timeout: timeout)
        defer { lock.release() }
        try emit(.object([("status", .string("reserved"))]), output: output)
        guard wait < .seconds(maximumSeconds) else {
            throw FlyrailValueError("invalid --wait-timeout: duration is too large")
        }
        let deadline = ContinuousClock.now.advanced(by: wait)
        while try NativeFilesystem.metadata(barrier) == nil {
            guard ContinuousClock.now < deadline else { throw FlyrailValueError("barrier timed out") }
            Thread.sleep(forTimeInterval: 0.005)
        }
        do {
            try ResourceAdmission.admit(reference, store: store)
            try emit(.object([("status", .string("admitted"))]), output: output)
        } catch let error as TargetError {
            try emit(.object([("status", .string(error.code.rawValue))]), output: output)
        }
    }

    private static func observe(_ options: [String: String], output: FileHandle) throws {
        do {
            let reference = try resource(options)
            let observed = try NativeObservation.snapshot(reference.destination)
            try emit(.object([
                ("resource", reference.stateRecord()), ("revision", observed.revision.stateRecord()),
                ("ancestors", StateCodec.sequence(observed.ancestors)),
                ("error", .null),
            ]), output: output)
        } catch let error as TargetError {
            try emit(.object([("error", .object([
                ("code", .string(error.code.rawValue)), ("message", .string(error.message)),
                ("path", error.path.map { .string($0.displayString) } ?? .null),
                ("errno", error.errno.map { .number(String($0)) } ?? .null),
            ]))]), output: output)
        }
    }

    private static func records(_ options: [String: String], output: FileHandle) throws {
        let store = try DirectoryStore(ResourcePath(required(options, "--state")))
        let result = try StateRecords.keys(store, prefix: "").compactMap { key -> JSONValue? in
            guard let bytes = try StateRecords.raw(store, key: key) else { return nil }
            let canonical = try StateCodec.transcode(bytes)
            guard let tag = try stateRecordTag(JSONParser.parse(canonical)) else {
                throw FlyrailValueError("record type missing: \(key)")
            }
            return .object([
                ("key", .string(key)), ("type", .string(tag)),
                ("bytes", .string(bytes.base64EncodedString())),
                ("canonical", .string(canonical.base64EncodedString())),
            ])
        }
        try emit(.object([("records", .array(result))]), output: output)
    }
}
