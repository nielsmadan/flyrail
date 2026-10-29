import Foundation

public protocol SkillTarget: Sendable {
    var root: ResourcePath { get }
    var agent: Agent? { get }
    var skillRoutingContext: [(String, String)] { get }
}

public extension SkillTarget {
    var agent: Agent? { nil }
    var skillRoutingContext: [(String, String)] { [("agent", ""), ("scope", "directory"), ("home", "")] }
}

extension Target: SkillTarget {
    public var skillRoutingContext: [(String, String)] { routingContext }
}

extension ResourcePath: SkillTarget {
    public var root: ResourcePath { self }
}

package let droidUnsupported = "Flyrail detects droid but has no verified configuration translation."

public func renderSkills(_ bundle: FlyrailBundle, target: some SkillTarget) throws -> RenderedBundle {
    try SkillInspection.validate(bundle)
    if target.agent == .droid {
        return try RenderedBundle(artifacts: [], notices: bundle.artifacts.compactMap {
            guard case let .skill(skill) = $0 else { return nil }
            return try Notice(
                artifactID: skill.name,
                kind: .unsupported,
                message: droidUnsupported,
                code: "agent-unsupported"
            )
        })
    }
    return try SkillInspection.render(bundle, root: NativeAuthority.canonicalDestination(target.root))
}

package enum SkillInspection {
    package static func validate(_ bundle: FlyrailBundle) throws {
        guard bundle.artifacts.allSatisfy({ if case .skill = $0 { true } else { false } }) else {
            throw FlyrailValueError("use explicit rendering for non-skill artifacts")
        }
        guard bundle.assets.isEmpty, bundle.dependencies.isEmpty else {
            throw FlyrailValueError("use explicit rendering for skill bundles with assets or dependencies")
        }
    }

    package static func render(_ bundle: FlyrailBundle, root: ResourcePath) throws -> RenderedBundle {
        try RenderedBundle(artifacts: bundle.artifacts.compactMap {
            guard case let .skill(skill) = $0 else { return nil }
            return try RenderedArtifact(id: skill.name, family: .skills, destination: root,
                                        content: .tree(skill.content), subtree: skill.name)
        })
    }

    package static func target(_ target: some SkillTarget,
                               bundleID: String) throws -> (ResourcePath, InstallationTarget)
    {
        if target
            .agent == .droid { throw TargetError(code: .unsupported, message: droidUnsupported, path: target.root) }
        try validateIdentifier(bundleID, label: "bundle id")
        let root = try NativeAuthority.canonicalDestination(target.root)
        let parts = try NativeFilesystem.components(root)
        let parent = try ResourcePath(platformBytes: [47] + parts.dropLast().joined(separator: [47]))
        let name = String(decoding: parts.last ?? [], as: UTF8.self)
        let index = try appendPath(parent, ".\(name).flyrail-index-\(bundleID)")
        let context = [("destination", root.displayString)]
        return try (root, InstallationTarget(indexRoot: index, context: context,
                                             routingContext: context + target.skillRoutingContext))
    }

    package static func observation(_ observed: InstallationObservation, bundle: FlyrailBundle?,
                                    rendered: RenderedBundle?, conflicts: [Conflict] = [],
                                    previewError: TargetError? = nil) throws -> Observation
    {
        var installations: [Installation] = []
        var metadataError: TargetError?
        for resource in observed.resources {
            do {
                guard let receipt = try NativeObservation.read(appendPath(resource.stateRoot, "receipt.json"),
                                                               as: Receipt.self, tag: "Receipt") else { continue }
                for identifier in Set(receipt.claims.map(\.bundleID)).sorted(by: utf8Less) {
                    let claims = receipt.claims.filter { $0.bundleID == identifier }
                    guard let first = claims.first else { continue }
                    let entries = try claims.flatMap { claim -> [BundleEntry] in
                        guard let installed = claim.installed,
                              case let .subtree(selector) = claim.claim.selector else { return [] }
                        return try installed.nodes.map { node in
                            try BundleEntry(path: node.path.isEmpty ? selector : selector + "/" + node.path,
                                            data: node.data,
                                            executable: node.data != nil && (node.mode.uint32 ?? 0) & 0o111 != 0)
                        }
                    }
                    try installations.append(Installation(bundleID: identifier, version: first.version,
                                                          contentDigest: first.bundleDigest,
                                                          transactionID: receipt.transactionID,
                                                          entries: inventory(entries)))
                }
            } catch { metadataError = NativeTransactionExecution.detail(error) }
        }
        let installed = installations.first { $0.bundleID == observed.bundleID }
        let modifications = try observed.resources.flatMap(\.claims).filter { $0.status != .current }.map {
            try Modification(bundleID: $0.bundleID, path: $0.artifactID, kind: .contentChanged)
        }
        let paths = observed.resources.flatMap(\.recoveryPaths)
        let error = observed.error ?? metadataError ?? observed.resources.compactMap(\.error).first ?? previewError
        let state: ObservationState = !paths.isEmpty ? .recoveryNeeded :
            (error != nil ? .unknown : (installed == nil ? .absent : .installed))
        return Observation(state: state, installed: installed, installations: installations,
                           versionMatches: bundle.flatMap { bundle in installed.map { exactTextEqual(
                               $0.version,
                               bundle.identity.version
                           ) } },
                           recordedContentMatches: bundle
                               .flatMap { bundle in installed.map { $0.contentDigest == bundle.contentDigest } },
                           contentMatches: bundle.map {
                               observed.isCurrent && observed.bundleDigest == $0.contentDigest &&
                                   (rendered == nil || observed.renderDigest == rendered?.renderDigest)
                           }, modifications: modifications, conflicts: conflicts, error: error, recoveryPaths: paths)
    }
}

public func inspect<T: SkillTarget>(_ bundle: FlyrailBundle, targets: [T]) -> [TargetInspection<T>] {
    var results: [TargetInspection<T>] = []
    var known: [ResourcePath: Int] = [:]
    for target in targets {
        var root = target.root
        var observation: Observation
        do {
            try SkillInspection.validate(bundle)
            let prepared = try SkillInspection.target(target, bundleID: bundle.identity.id)
            root = prepared.0
            if let alias = known[root] {
                let prior = results[alias]
                results.append(TargetInspection(target: target, root: root, stateRoot: prior.stateRoot,
                                                aliasOf: alias, observation: prior.observation))
                continue
            }
            known[root] = results.count
            let rendered = try SkillInspection.render(bundle, root: root)
            let observed = try inspectInstallation(bundleID: bundle.identity.id, target: prepared.1)
            let proposed = try preview(bundle: bundle, rendered: rendered, target: prepared.1)
            let conflicts = proposed.resources.filter { $0.error?.code == .conflict }.map {
                Conflict(path: $0.resource.destination.displayString)
            }
            let error = proposed.error ?? proposed.resources.compactMap(\.error).first { $0.code != .conflict }
            observation = try SkillInspection.observation(observed, bundle: bundle, rendered: rendered,
                                                          conflicts: conflicts, previewError: error)
        } catch { observation = .unknown(NativeTransactionExecution.detail(error)) }
        results.append(TargetInspection(target: target, root: root, stateRoot: (try? statePath(for: root)) ?? root,
                                        observation: observation))
    }
    return results
}
