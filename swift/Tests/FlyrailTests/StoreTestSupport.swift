@testable import Flyrail
import Foundation
import Testing

struct NativeTestRoot {
    let url: URL
    let path: ResourcePath
    let stateURL: URL
    let store: DirectoryStore

    init() throws {
        let name = UUID().uuidString
        let parent = stateRepositoryRoot().appending(path: ".cache/swift-native-tests")
        url = parent.appending(path: name)
        stateURL = parent.appending(path: "\(name)-store")
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        path = try ResourcePath(url.path)
        store = try DirectoryStore(ResourcePath(stateURL.path))
    }

    func child(_ name: String) throws -> ResourcePath {
        try appendPath(path, name)
    }

    func target(context: [(String, String)] = [],
                routingContext: [(String, String)] = []) throws -> InstallationTarget
    {
        try InstallationTarget(store: store, context: context, routingContext: routingContext)
    }

    func cleanup() throws {
        if FileManager.default.fileExists(atPath: stateURL.path) {
            try FileManager.default.removeItem(at: stateURL)
        }
        try FileManager.default.removeItem(at: url)
    }
}

func expectNativeError(_ code: ErrorCode, _ body: () throws -> Void) {
    do {
        try body()
        Issue.record("expected \(code.rawValue)")
    } catch let error as TargetError {
        #expect(error.code == code)
    } catch {
        Issue.record("unexpected error: \(error)")
    }
}

func transactionBundle(_ version: String = "1", owner: String = "team") throws -> FlyrailBundle {
    try FlyrailBundle(
        identity: BundleIdentity(id: owner, version: version),
        artifacts: [.instruction(InstructionArtifact(id: "guide", text: "text"))],
        assets: [], dependencies: [],
    )
}

func transactionRendered(_ path: ResourcePath, _ text: String = "generated\n") throws -> RenderedBundle {
    try RenderedBundle(artifacts: [
        RenderedArtifact(id: "guide", family: .instructions, destination: path,
                         content: .file(FileContent(data: Data(text.utf8), mode: .readable))),
    ])
}

func lifecycleSection(_ path: ResourcePath, text: String = "generated", id: String = "guide") throws -> RenderedBundle {
    try RenderedBundle(artifacts: [
        RenderedArtifact(id: id, family: .instructions, destination: path,
                         content: .section(SectionContent(marker: "guide", text: text))),
    ])
}

func transactionWrite(
    _ bundle: FlyrailBundle, rendered: RenderedBundle, target: InstallationTarget,
    acquisition: Acquisition = .conflict, replaceModified: Bool = false,
    seam: NativeTransactionSeam = .none,
) throws -> InstallationResult {
    try LifecycleExecution.synchronize(
        bundleID: bundle.identity.id, bundle: bundle, rendered: rendered, target: target,
        acquisition: acquisition, replaceModified: replaceModified, lockTimeout: .zero, seam: seam,
    )
}

func transactionRemove(_ owner: String, target: InstallationTarget,
                       seam: NativeTransactionSeam = .none) throws -> InstallationResult
{
    try LifecycleExecution.synchronize(
        bundleID: owner, bundle: nil, rendered: .empty, target: target, acquisition: .conflict,
        replaceModified: false, lockTimeout: .zero, seam: seam,
    )
}

func transactionRecover(_ target: InstallationTarget, owner: String = "team",
                        seam: NativeTransactionSeam = .none) throws -> InstallationResult
{
    do {
        guard let index = try InstallationIndex.read(bundleID: owner, target: target) else {
            return try InstallationResult(status: .unchanged,
                                          observation: InstallationInspection.observe(bundleID: owner, target: target))
        }
        let references = try LifecyclePlanning.references(.empty, index: index, target: target, bundle: nil,
                                                          group: NativeTransactionPlanning.group(.empty))
        let locks = try LifecycleExecution.locks(bundleID: owner, target: target, expected: index,
                                                 resources: references, timeout: .zero)
        defer { locks.release() }
        return try LifecycleExecution.recover(bundleID: owner, target: target, locks: locks, seam: seam)
    } catch let crash as NativeSimulatedCrash {
        throw crash
    } catch {
        return InstallationInspection.failed(owner, target: target, error: error)
    }
}

func randomTransactionID() -> String {
    UUID().uuidString.lowercased().filter { $0 != "-" }
}

func storePlan(_ ref: ResourceRef, store: any StateStore, after: Revision, previous: Receipt?) throws -> ResourcePlan {
    _ = try NativeTransactionDirectory.createStoreParents(store)
    let transactionID = randomTransactionID()
    let directory = try NativeTransactionDirectory.choose(
        store: store, destination: ref.destination, transactionID: transactionID,
    )
    return try ResourcePlan(
        resource: ref, before: NativeObservation.snapshot(ref.destination).revision, after: after,
        previous: previous, receipt: Receipt(resource: ref, transactionID: transactionID),
        ancestors: ResourceTransaction.planAncestors(destination: ref.destination, directory: directory),
        transactionDirectory: directory, recoveryPaths: [],
    )
}

private final class CrashRecord {
    var fired = false
}

func crashSeam(at boundary: String) -> (NativeTransactionSeam, fired: () -> Bool) {
    let record = CrashRecord()
    let seam = NativeTransactionSeam { selected in
        guard selected == boundary, !record.fired else { return }
        record.fired = true
        throw NativeSimulatedCrash(boundary)
    }
    return (seam, { record.fired })
}

func transactionRecordKeys(_ store: any StateStore) throws -> [String] {
    try store.keys(prefix: "resource/").filter { $0.hasSuffix("/preparation") || $0.hasSuffix("/journal") }
}

func storedPreparation(_ destination: ResourcePath, store: any StateStore) throws -> Preparation? {
    try StateRecords.read(
        store, key: StateKey.resource(destination, .preparation), as: Preparation.self, tag: "Preparation",
    )
}

func storedReceipt(_ destination: ResourcePath, store: any StateStore) throws -> Receipt? {
    try StateRecords.read(store, key: StateKey.resource(destination, .receipt), as: Receipt.self, tag: "Receipt")
}

func storeContents(_ store: any StateStore) throws -> [String: Data?] {
    try Dictionary(uniqueKeysWithValues: store.keys(prefix: "").map { try ($0, store.read($0)) })
}
