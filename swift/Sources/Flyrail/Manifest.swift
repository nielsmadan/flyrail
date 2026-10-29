import Foundation

package func readBundleManifest(_ data: Data) throws -> (BundleIdentity, [SkillSpec]) {
    guard !data.starts(with: [0xEF, 0xBB, 0xBF]) else {
        throw FlyrailValueError("manifest is not valid UTF-8")
    }
    let raw: JSONValue
    do {
        raw = try JSONParser.parse(data)
    } catch let error as FlyrailValueError where error.message == "JSON must be UTF-8" {
        throw FlyrailValueError("manifest is not valid UTF-8")
    }
    let manifest = try manifestObject(
        raw,
        required: ["schema_version", "id", "version", "skills"],
        optional: []
    )
    guard manifestValue(manifest, named: "schema_version") == .number("1") else {
        throw FlyrailValueError("schema_version must be the integer 1")
    }
    let identity = try BundleIdentity(
        id: manifestString(manifestValue(manifest, named: "id"), label: "bundle id"),
        version: manifestString(manifestValue(manifest, named: "version"), label: "version")
    )
    guard case let .array(records)? = manifestValue(manifest, named: "skills"), !records.isEmpty else {
        throw FlyrailValueError("skills must be a nonempty JSON array")
    }
    var specs: [SkillSpec] = []
    var names: Set<[UInt8]> = []
    var paths: [String] = []
    for record in records {
        let fields = try manifestObject(record, required: ["name", "path"], optional: ["executables"])
        let name = try manifestString(manifestValue(fields, named: "name"), label: "skill name")
        let path = try manifestString(manifestValue(fields, named: "path"), label: "skill path")
        let executables: [String]
        if let rawExecutables = manifestValue(fields, named: "executables") {
            guard case let .array(items) = rawExecutables else {
                throw FlyrailValueError("executables must be a JSON array")
            }
            executables = try items.map { try manifestString($0, label: "executable path") }
        } else {
            executables = []
        }
        let spec = try SkillSpec(name: name, path: path, executables: executables)
        guard names.insert(Array(name.utf8)).inserted else {
            throw FlyrailValueError("duplicate skill name: \(name)")
        }
        let key = portablePathKey(path)
        guard !paths.contains(where: { pathsOverlap(key, $0) }) else {
            throw FlyrailValueError("skill source paths must not overlap")
        }
        paths.append(key)
        specs.append(spec)
    }
    return (identity, specs)
}

private func manifestObject(
    _ value: JSONValue,
    required: [String],
    optional: [String]
) throws -> [(String, JSONValue)] {
    guard case let .object(items) = value else {
        throw FlyrailValueError("manifest and skill records must be JSON objects")
    }
    let allowed = required + optional
    let unknown = items.contains { item in
        !allowed.contains(where: { exactTextEqual($0, item.0) })
    }
    let missing = required.contains { field in
        !items.contains(where: { exactTextEqual($0.0, field) })
    }
    guard !unknown, !missing else {
        throw FlyrailValueError(
            "expected fields \(fieldList(required)); optional fields \(fieldList(optional))"
        )
    }
    return items
}

private func manifestValue(
    _ items: [(String, JSONValue)],
    named name: String
) -> JSONValue? {
    items.first(where: { exactTextEqual($0.0, name) })?.1
}

private func manifestString(_ value: JSONValue?, label: String) throws -> String {
    guard case let .string(text)? = value else {
        throw FlyrailValueError("\(label) must be a string")
    }
    return text
}

private func fieldList(_ fields: [String]) -> String {
    "[" + fields.sorted().map { "'\($0)'" }.joined(separator: ", ") + "]"
}

private func pathsOverlap(_ left: String, _ right: String) -> Bool {
    left == right || left.hasPrefix("\(right)/") || right.hasPrefix("\(left)/")
}
