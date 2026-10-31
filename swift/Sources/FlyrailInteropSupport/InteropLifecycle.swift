import Flyrail
import Foundation

extension InteropDifferential {
    package static func lifecycleRecords(_ rows: [LifecycleRow], root: URL) throws -> [String] {
        try rows.flatMap { try lifecycleCase($0, root: root) }
    }

    private static func lifecycleCase(_ row: LifecycleRow, root: URL) throws -> [String] {
        let name = row.name
        let caseRoot = root.appending(path: name)
        try FileManager.default.createDirectory(at: caseRoot, withIntermediateDirectories: false)
        let target = try InstallationTarget(store: DirectoryStore(ResourcePath(
            root.appending(path: ".state").appending(path: name).path,
        )))
        let (first, ids) = try lifecycleRendered(root: caseRoot, kind: row.kind, text: row.first)
        if let initial = row.initial {
            try initial.write(to: URL(fileURLWithPath: first.artifacts[0].destination.displayString))
        }
        let bundle = try lifecycleBundle(version: "1", ids: ids)
        var records = try stepRecords(name, step: "1-create",
                                      result: sync(bundle: bundle, rendered: first, target: target,
                                                   acquisition: row.acquisition),
                                      root: caseRoot, target: target)
        try records += stepRecords(name, step: "2-unchanged",
                                   result: sync(bundle: bundle, rendered: first, target: target),
                                   root: caseRoot, target: target)
        let (second, _) = try lifecycleRendered(root: caseRoot, kind: row.kind, text: row.second)
        try records += stepRecords(name, step: "3-update",
                                   result: sync(bundle: lifecycleBundle(version: "2", ids: ids), rendered: second,
                                                target: target),
                                   root: caseRoot, target: target)
        try records += stepRecords(name, step: "4-recover",
                                   result: recoverInstallation(bundleID: "team", target: target),
                                   root: caseRoot, target: target)
        let observed = try inspectInstallation(bundleID: "team", target: target)
        try records.append(lifecycleRecord(name, field: "5-inspect", value: [
            "current": observed.isCurrent, "pending": observed.pending,
            "revision": observed.resources.map { optional($0.revisionDigest) },
        ]))
        try records += stepRecords(name, step: "6-remove", result: remove(bundleID: "team", target: target),
                                   root: caseRoot, target: target)
        return records
    }

    static func lifecycleBundle(version: String, ids: [String]) throws -> FlyrailBundle {
        try FlyrailBundle(identity: BundleIdentity(id: "team", version: version), artifacts: ids.map {
            try .instruction(InstructionArtifact(id: $0, text: $0))
        }, assets: [], dependencies: [])
    }

    static func lifecycleRendered(root: URL, kind: LifecycleKind, text: String) throws -> (RenderedBundle, [String]) {
        let filename = switch kind {
        case .file, .alias: "owned"
        case .casefold: "Straße"
        case .toml: "config.toml"
        case .json: "config.json"
        case .section, .shared: "AGENTS.md"
        }
        let destination = try ResourcePath(root.appending(path: filename).path)
        let ids = ["alpha", "zeta"]
        let content: Content
        switch kind {
        case .shared:
            let artifacts = try ids.map { id in
                try RenderedArtifact(id: id, family: .instructions, destination: destination,
                                     content: .section(SectionContent(marker: id, text: "\(id)-\(text)\n")))
            }
            return try (RenderedBundle(artifacts: artifacts.reversed()), ids)
        case .alias:
            let artifacts = try ids.map { id in
                try RenderedArtifact(id: id, family: .instructions, destination: destination,
                                     content: .file(FileContent(data: Data(text.utf8), mode: .readable)))
            }
            return try (RenderedBundle(artifacts: artifacts), ids)
        case .file, .casefold: content = .file(FileContent(data: Data(text.utf8), mode: .readable))
        case .section: content = try .section(SectionContent(marker: "managed", text: text))
        case .json, .toml:
            content = try .structured(StructuredContent(format: kind == .json ? .json : .toml,
                                                        selector: Selector(parts: [.key(Key("tool"))]),
                                                        value: .scalar(.text(text))))
        }
        return try (RenderedBundle(artifacts: [
            RenderedArtifact(id: "managed", family: .instructions, destination: destination, content: content),
        ], routingContext: [("seed", "1592639710"), ("surface", "differential")]), ["managed"])
    }

    static func stepRecords(_ name: String, step: String, result: InstallationResult,
                            root: URL, target: InstallationTarget) throws -> [String]
    {
        var records = try [lifecycleRecord(name, field: step + "-result", value: resultValue(result, root: root))]
        let store = target.store
        for key in try StateRecords.keys(store, prefix: "") {
            guard let data = try StateRecords.raw(store, key: key) else { continue }
            guard try StateCodec.transcode(data) == data else {
                throw FlyrailValueError("state record is not canonical: \(key)")
            }
            let value = try JSONSerialization.jsonObject(with: data)
            let normalized = try sortedJSON(normalize(value, root: root)) + Data([10])
            try records.append(record("lifecycle", name, "\(step)-state-\(stateKey(key, value: value, root: root))",
                                      normalized))
        }
        let payloads = try regularFiles(root).filter {
            !$0.path.contains(".flyrail") && $0.lastPathComponent != "lock"
        }.map { try [relative($0.path, root: root), Data(contentsOf: $0).nativeHex] }
        try records.append(lifecycleRecord(name, field: step + "-payloads", value: payloads))
        return records
    }

    private static func stateKey(_ key: String, value: Any, root: URL) throws -> String {
        guard key.hasPrefix("resource/") else { return key }
        let record = value as? [String: Any]
        let fields = record?["fields"] as? [String: Any]
        let resource = record?["type"] as? String == "ResourceRef" ? fields :
            (fields?["resource"] as? [String: Any])?["fields"] as? [String: Any]
        guard let destination = (resource?["destination"] as? [String: Any])?["path"] as? String,
              let suffix = key.split(separator: "/").last
        else { throw FlyrailValueError("state record has no destination: \(key)") }
        return "resource/\(relative(destination, root: root))/\(suffix)"
    }

    private static func resultValue(_ result: InstallationResult, root: URL) -> [String: Any] {
        let observed = result.observation
        return [
            "error": errorValue(result.error, root: root),
            "observation": [
                "bundle_digest": optional(observed.bundleDigest), "current": observed.isCurrent,
                "error": errorValue(observed.error, root: root), "pending": observed.pending,
                "render_digest": optional(observed.renderDigest),
                "resources": observed.resources.map { resource -> [String: Any] in
                    [
                        "claims": resource.claims.map { claim -> [String: Any] in
                            ["artifact": claim.artifactID, "bundle": claim.bundleID,
                             "bundle_digest": claim.bundleDigest, "claim": claim.claimID,
                             "render_digest": claim.renderDigest, "status": claim.status.rawValue,
                             "version": claim.version]
                        },
                        "destination": relative(resource.destination.displayString, root: root),
                        "error": errorValue(resource.error, root: root),
                        "revision": optional(resource.revisionDigest),
                    ]
                },
                "version": optional(observed.version),
            ],
            "resources": result.resources.map {
                ["destination": relative($0.resource.destination.displayString, root: root),
                 "error": errorValue($0.error, root: root), "status": $0.status.rawValue]
            },
            "status": result.status.rawValue,
        ]
    }

    static func optional(_ value: (some Any)?) -> Any {
        value.map { $0 as Any } ?? NSNull()
    }

    private static func relative(_ path: String, root: URL) -> String {
        let prefix = root.path + "/"
        let text = path.hasPrefix(prefix) ? "$ROOT/" + path.dropFirst(prefix.count) : path
        return text.replacing(#/((?:^|/)(?:\.flyrail-)?[0-9a-f]{64}-)[0-9a-f]{32}(?=\.txn|/|$)/#) {
            $0.output.1 + String(repeating: "0", count: 32)
        }
    }

    private static func errorValue(_ error: TargetError?, root: URL) -> Any {
        guard let error else { return NSNull() }
        return ["code": error.code.rawValue, "message": error.message,
                "path": optional(error.path.map { relative($0.displayString, root: root) })]
    }

    private static func regularFiles(_ root: URL) throws -> [URL] {
        guard let enumerator = FileManager.default.enumerator(at: root,
                                                              includingPropertiesForKeys: [.isRegularFileKey])
        else { throw FlyrailValueError("cannot enumerate lifecycle outputs") }
        return try enumerator.compactMap { entry -> URL? in
            guard let url = entry as? URL,
                  try url.resourceValues(forKeys: [.isRegularFileKey]).isRegularFile == true else { return nil }
            return url
        }.sorted { utf8Less($0.path, $1.path) }
    }

    private static func normalize(_ value: Any, root: URL) -> Any {
        if let values = value as? [Any] {
            return values.map { normalize($0, root: root) }
        }
        guard let fields = value as? [String: Any] else { return value }
        var normalized = fields.mapValues { normalize($0, root: root) }
        if normalized.count == 1, let path = normalized["path"] as? String {
            normalized["path"] = relative(path, root: root)
        }
        if var record = normalized["fields"] as? [String: Any] {
            if ["Node", "Ancestor"].contains(normalized["type"] as? String ?? "") {
                for key in ["device", "inode", "uid", "gid"] {
                    record[key] = 0
                }
                record["security"] = ["bytes": ""]
            }
            if ["Receipt", "Preparation"].contains(normalized["type"] as? String ?? "") {
                record["transaction_id"] = String(repeating: "0", count: 32)
            }
            normalized["fields"] = record
        }
        return normalized
    }

    private static func sortedJSON(_ value: Any) throws -> Data {
        try JSONSerialization.data(withJSONObject: value, options: [.sortedKeys, .withoutEscapingSlashes])
    }

    static func lifecycleRecord(_ name: String, field: String, value: Any) throws -> String {
        try record("lifecycle", name, field, sortedJSON(value))
    }
}
