import Flyrail
import Foundation

package extension InteropDifferential {
    static func lifecycleEdgeRecords(root: URL) throws -> [String] {
        let name = "public-boundaries"
        let caseRoot = root.appending(path: name)
        try FileManager.default.createDirectory(at: caseRoot, withIntermediateDirectories: false)
        let store = try DirectoryStore(ResourcePath(root.appending(path: ".state").appending(path: name).path))
        let target = try InstallationTarget(store: store)
        let (rendered, ids) = try lifecycleRendered(root: caseRoot, kind: .shared, text: "one")
        let bundle = try lifecycleBundle(version: "1", ids: ids)
        let created = try sync(bundle: bundle, rendered: rendered, target: target)
        guard created.status == .applied else {
            throw FlyrailValueError("lifecycle edge setup was \(created.status.rawValue)")
        }
        let observed = try inspectInstallation(bundleID: "team", target: target)
        let (changed, _) = try lifecycleRendered(root: caseRoot, kind: .shared, text: "two")
        let before = try NativeObservation.snapshot(ResourcePath(caseRoot.path))
        let stored = try storeSnapshot(store)
        let matches = try [observed.matches(bundle, rendered),
                           observed.matches(lifecycleBundle(version: "2", ids: ids), rendered),
                           observed.matches(bundle, changed)]
        let pure = try NativeObservation.snapshot(ResourcePath(caseRoot.path)) == before &&
            storeSnapshot(store) == stored
        var records = try [lifecycleRecord(name, field: "desired-generation",
                                           value: ["matches": matches, "pure": pure])]
        let proposed = try preview(bundle: lifecycleBundle(version: "2", ids: ids), rendered: changed, target: target)
        let key = try InstallationIndex.key(bundleID: "team", target: target)
        guard let index = try InstallationIndex.read(bundleID: "team", target: target) else {
            throw FlyrailValueError("lifecycle edge index is missing")
        }
        let pending = try Index(bundleID: "team", contextDigest: target.contextDigest,
                                pending: index.current, residual: index.resources)
        try StateRecords.write(store, key: key, value: pending, expected: index)
        let stale = applyPreview(proposed)
        try records.append(lifecycleRecord(name, field: "stale-preview", value: [
            "status": stale.status.rawValue, "error": optional(stale.error?.code.rawValue),
            "retained": StateRecords.raw(store, key: key) == StateCodec.encode(pending),
        ]))
        try records += stepRecords(name, step: "source-free-recover",
                                   result: recoverInstallation(bundleID: "team", target: target),
                                   root: caseRoot, target: target)
        let partial = try inspectInstallation(bundleID: "team", target: target)
        try records.append(lifecycleRecord(name, field: "partial-inspect",
                                           value: ["current": partial.isCurrent, "pending": partial.pending]))
        try records += stepRecords(name, step: "partial-sync",
                                   result: sync(bundle: bundle, rendered: rendered, target: target),
                                   root: caseRoot, target: target)
        try records += stepRecords(name, step: "source-free-remove", result: remove(bundleID: "team", target: target),
                                   root: caseRoot, target: target)
        return records
    }

    private static func storeSnapshot(_ store: any StateStore) throws -> [[Data]] {
        try StateRecords.keys(store, prefix: "").map { try [Data($0.utf8), StateRecords.raw(store, key: $0) ?? Data()] }
    }
}
