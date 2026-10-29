import Darwin
import Foundation

package enum InteropCommand: String, Sendable {
    case differential
    case encode
    case decode
    case write
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
        case .hold: allowed = ["--destination", "--kind", "--barrier", "--lock-timeout", "--wait-timeout"]
        case .observe: allowed = ["--destination", "--kind", "--repeat"]
        case .records: allowed = ["--index", "--destination"]
        case .write:
            allowed = ["--index", "--bundle-id", "--lock-timeout", "--destination", "--version",
                       "--payload", "--large", "--mode", "--acquisition", "--replace-modified",
                       "--fail-index-completion", "--barrier-phase", "--barrier-ready", "--barrier-release",
                       "--barrier-timeout", "--fault-phase", "--kind", "--section", "--source", "--agent", "--node"]
        case .inspect: allowed = ["--index", "--bundle-id", "--lock-timeout"]
        case .recover:
            allowed = ["--index", "--bundle-id", "--lock-timeout", "--barrier-phase", "--barrier-ready",
                       "--barrier-release", "--barrier-timeout", "--fault-phase"]
        case .remove: allowed = ["--index", "--bundle-id", "--lock-timeout", "--replace-modified"]
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
        case .write, .inspect, .remove, .recover: try operate(command, options: options, output: output)
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

    private static func timeout(_ options: [String: String], _ key: String,
                                default fallback: Double) throws -> Duration
    {
        guard let seconds = Double(options[key] ?? String(fallback)), seconds.isFinite, seconds >= 0 else {
            throw FlyrailValueError(
                "\(key.dropFirst(2).replacingOccurrences(of: "-", with: " ")) must be finite and nonnegative"
            )
        }
        guard seconds <= 1e9 else { throw FlyrailValueError("invalid \(key): duration is too large") }
        return .seconds(seconds)
    }

    private static func emit(_ value: JSONValue, output: FileHandle) throws {
        try output.write(contentsOf: StateCodec.encodeRecord(value))
    }

    private static func hold(_ options: [String: String], output: FileHandle) throws {
        let reference = try resource(options)
        let barrier = try NativeAuthority.absolute(ResourcePath(required(options, "--barrier")))
        let timeout = try timeout(options, "--lock-timeout", default: 0)
        let wait = try self.timeout(options, "--wait-timeout", default: 30)
        let lock = try NativeAdmission.reserve(reference, timeout: timeout)
        try emit(.object([("status", .string("reserved"))]), output: output)
        let deadline = ContinuousClock.now.advanced(by: wait)
        while try NativeFilesystem.metadata(barrier) == nil {
            guard ContinuousClock.now < deadline else { throw FlyrailValueError("barrier timed out") }
            Thread.sleep(forTimeInterval: 0.005)
        }
        do {
            try NativeAdmission.admit(reference, lock: lock)
            try emit(.object([("status", .string("admitted"))]), output: output)
        } catch let error as TargetError {
            try emit(.object([("status", .string(error.code.rawValue))]), output: output)
        }
    }

    private static func observe(_ options: [String: String], output: FileHandle) throws {
        do {
            let reference = try resource(options)
            let observed = try NativeAdmission.observe(reference)
            try emit(.object([
                ("resource", reference.stateRecord()), ("revision", observed.revision.stateRecord()),
                ("state_revision", observed.state.revision.stateRecord()),
                ("ancestors", StateCodec.sequence(observed.snapshot.ancestors)),
                ("receipt", StateCodec.optional(observed.receipt)),
                ("recovery_paths", .array(observed.recovery.map { .string($0.displayString) })),
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
        let index = try InstallationTarget(indexRoot: ResourcePath(required(options, "--index")))
        var roots = [index.indexRoot]
        if let destination = options["--destination"] {
            try roots.append(ResourceAuthority(destination: ResourcePath(destination)).stateRoot)
        }
        let recordNames: Set<String> = [
            "authority.json", "authority.json.next", "receipt.json", "receipt.json.next",
            "transaction.json", "transaction.json.next", "index.json", "index.json.next",
        ]
        var records: [(ResourcePath, Data)] = []
        for root in roots {
            try NativeAuthority.managedSpelling(root)
            let snapshot = try NativeObservation.snapshot(
                root, privateStorage: true, resourcePayloads: root != index.indexRoot
            )
            for node in snapshot.revision.nodes.dropFirst() where recordNames.contains(node.path) {
                guard let bytes = node.data else {
                    throw try nativeFailure(
                        .invalidState,
                        "metadata must be a regular file",
                        appendPath(root, node.path)
                    )
                }
                try records.append((appendPath(root, node.path), bytes))
            }
            try NativeObservation.require(root, snapshot)
        }
        let result = try records.sorted { $0.0.platformBytes.lexicographicallyPrecedes($1.0.platformBytes) }
            .map { path, bytes -> JSONValue in
                let canonical = try StateCodec.transcode(bytes)
                guard let tag = try stateRecordTag(JSONParser.parse(canonical)) else {
                    throw FlyrailValueError("record type missing")
                }
                return .object([
                    ("path", .string(path.displayString)), ("type", .string(tag)),
                    ("bytes", .string(bytes.base64EncodedString())),
                    ("canonical", .string(canonical.base64EncodedString())),
                ])
            }
        try emit(.object([("records", .array(result))]), output: output)
    }
}
