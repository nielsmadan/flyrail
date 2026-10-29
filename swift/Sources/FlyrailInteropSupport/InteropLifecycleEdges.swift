import Flyrail
import Foundation

package extension InteropDifferential {
    static func lifecycleEdgeRecords(root: URL) throws -> [String] {
        let name = "public-boundaries"
        let caseRoot = root.appending(path: name)
        try FileManager.default.createDirectory(at: caseRoot, withIntermediateDirectories: false)
        let target = try InstallationTarget(indexRoot: ResourcePath(caseRoot.appending(path: "index").path))
        let (rendered, ids) = try lifecycleRendered(root: caseRoot, kind: .shared, text: "one")
        let bundle = try lifecycleBundle(version: "1", ids: ids)
        let created = try sync(bundle: bundle, rendered: rendered, target: target)
        guard created.status == .applied else {
            throw FlyrailValueError("lifecycle edge setup was \(created.status.rawValue)")
        }
        let observed = try inspectInstallation(bundleID: "team", target: target)
        let (changed, _) = try lifecycleRendered(root: caseRoot, kind: .shared, text: "two")
        let before = try NativeObservation.snapshot(ResourcePath(caseRoot.path))
        let matches = try [observed.matches(bundle, rendered),
                           observed.matches(lifecycleBundle(version: "2", ids: ids), rendered),
                           observed.matches(bundle, changed)]
        let pure = try NativeObservation.snapshot(ResourcePath(caseRoot.path)) == before
        var records = try [lifecycleRecord(name, field: "desired-generation",
                                           value: ["matches": matches, "pure": pure])]
        let proposed = try preview(bundle: lifecycleBundle(version: "2", ids: ids), rendered: changed, target: target)
        let indexURL = caseRoot.appending(path: "index/index.json")
        let nextURL = caseRoot.appending(path: "index/index.json.next")
        try FileManager.default.copyItem(at: indexURL, to: nextURL)
        let preparation = try Data(contentsOf: nextURL)
        let stale = applyPreview(proposed)
        try records.append(lifecycleRecord(name, field: "stale-no-recovery", value: [
            "status": stale.status.rawValue, "error": optional(stale.error?.code.rawValue),
            "retained": Data(contentsOf: nextURL) == preparation,
        ]))
        try records += stepRecords(name, step: "source-free-recover",
                                   result: recoverInstallation(bundleID: "team", target: target),
                                   root: caseRoot, target: target)
        guard let index = try NativeAdmission.observeIndex(target, bundleID: "team").current else {
            throw FlyrailValueError("lifecycle edge index is missing")
        }
        let pending = try Index(bundleID: "team", contextDigest: target.contextDigest,
                                pending: index.current, residual: index.resources)
        try StateCodec.encode(pending).write(to: indexURL)
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
}
