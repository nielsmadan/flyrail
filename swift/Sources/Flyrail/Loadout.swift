import Foundation

public struct LoadoutDiagnostic: Error, Sendable, Hashable {
    public let code: String
    public let message: String
    public let path: ResourcePath

    package init(_ code: String, _ message: String, _ path: ResourcePath) {
        self.code = code
        self.message = message
        self.path = path
    }
}

public struct LoadoutSource: Sendable, Hashable {
    public let name: String
    public let path: ResourcePath
    public let uses: Set<String>
    public let declaredIn: ResourcePath
    public let overrides: [String: Set<String>]
}

public struct LoadoutHarness: Sendable, Hashable {
    public let agent: Agent
    public let slices: Set<String>
}

public struct LoadoutSelector: Sendable, Hashable {
    public let agent: Agent
    public let category: String
    public let manifest: ResourcePath
    public let key: [String]
    public let entries: [String]
    public let substitutions: [String: String]
    package let array: Bool
    package let destinations: Int
    package let allProfiles: Bool
}

public enum LoadoutDiscovery: Sendable {
    case absent(registry: ResourcePath)
    case available(LoadoutSetup)
    case invalid(registry: ResourcePath, diagnostic: LoadoutDiagnostic)
}

public struct LoadoutSetup: Sendable {
    public let registry: ResourcePath
    public let root: ResourcePath
    public let profile: String
    package let home: ResourcePath
    package let manifest: LoadoutManifest
    package let conditions: [LoadoutRead]

    public var profileChain: [ResourcePath] {
        manifest.chain
    }

    public var sources: [LoadoutSource] {
        manifest.sources
    }

    public var harnesses: [LoadoutHarness] {
        manifest.harnesses
    }

    public var selectors: [LoadoutSelector] {
        manifest.selectors
    }

    package func hasSlice(_ item: LoadoutSlice) -> Bool {
        harnesses.contains { $0.agent == item.agent && $0.slices.contains(item.category) }
    }

    public func eligibleSources(bundle: FlyrailBundle, harnesses: [Agent]) -> [LoadoutSource] {
        let needed = loadoutRequiredSlices(bundle, harnesses: harnesses)
        guard !harnesses.isEmpty, loadoutHooksSupported(bundle, harnesses: harnesses),
              needed.allSatisfy({ harnesses.contains($0.agent) && hasSlice($0) && !manifest.customRoutes.contains($0) })
        else {
            return []
        }
        return sources.filter { source in needed.allSatisfy { source.uses.contains($0.category) } }
    }

    public func select(
        source name: String, harnesses: [Agent], acceptSharedScope: Bool = false,
    ) throws -> LoadoutDestination {
        guard let source = sources.first(where: { exactTextEqual($0.name, name) }) else {
            throw FlyrailValueError("Select a declared Loadout source, found no source \(name)")
        }
        guard !harnesses.isEmpty, Set(harnesses).count == harnesses.count else {
            throw FlyrailValueError("Select one or more distinct Loadout harnesses")
        }
        return LoadoutDestination(setup: self, source: source,
                                  harnesses: harnesses.sorted { $0.rawValue < $1.rawValue },
                                  acceptSharedScope: acceptSharedScope)
    }
}

public struct LoadoutDestination: Sendable {
    public let setup: LoadoutSetup
    public let source: LoadoutSource
    public let harnesses: [Agent]
    public let acceptSharedScope: Bool

    public var location: LoadoutLocation {
        LoadoutLocation(
            discoveredRoot: setup.root,
            profile: setup.profile,
            sourceName: source.name,
            sourcePath: source.path,
        )
    }
}

public struct LoadoutLocation: Sendable, Hashable {
    public let root: ResourcePath
    public let profile: String
    public let sourceName: String
    public let sourcePath: ResourcePath

    public init(root: ResourcePath, profile: String, sourceName: String, sourcePath: ResourcePath) throws {
        _ = try loadoutAbsolute(root)
        _ = try loadoutAbsolute(sourcePath)
        guard (try? loadoutProfileFile(root, profile)) != nil else {
            throw FlyrailValueError("Profile must name a sibling manifest")
        }
        guard !sourceName.isEmpty, !sourceName.contains("/"), !sourceName.contains("\\"),
              !sourceName.utf8.contains(0)
        else {
            throw FlyrailValueError("Select one Loadout source name")
        }
        self.init(discoveredRoot: root, profile: profile, sourceName: sourceName, sourcePath: sourcePath)
    }

    fileprivate init(discoveredRoot: ResourcePath, profile: String, sourceName: String, sourcePath: ResourcePath) {
        root = discoveredRoot
        self.profile = profile
        self.sourceName = sourceName
        self.sourcePath = sourcePath
    }

    public func installation(store: any StateStore) throws -> InstallationTarget {
        try InstallationTarget(store: store, context: [
            ("destination", "loadout"), ("root", root.displayString), ("profile", profile),
            ("source-name", sourceName), ("source-path", sourcePath.displayString),
        ])
    }

    public static func == (left: Self, right: Self) -> Bool {
        left.root == right.root && left.sourcePath == right.sourcePath &&
            exactTextEqual(left.profile, right.profile) && exactTextEqual(left.sourceName, right.sourceName)
    }

    public func hash(into hasher: inout Hasher) {
        hasher.combine(root)
        hasher.combine(sourcePath)
        hashExactText(profile, into: &hasher)
        hashExactText(sourceName, into: &hasher)
    }
}

public struct LoadoutImpact: Sendable, Hashable {
    public let category: String
    public let path: ResourcePath
    public let harnesses: [Agent]
    public let shared: Bool
}

public struct LoadoutOptions: Sendable, Hashable {
    public let platform: Platform
    public let hookRuntime: HookRuntime?
    public let executionRoot: ResourcePath?
    public let replaceModified: Bool

    public init(platform: Platform, hookRuntime: HookRuntime? = nil, executionRoot: ResourcePath? = nil,
                replaceModified: Bool = false)
    {
        self.platform = platform
        self.hookRuntime = hookRuntime
        self.executionRoot = executionRoot
        self.replaceModified = replaceModified
    }
}

public struct LoadoutPreview: Sendable, Hashable {
    public let location: LoadoutLocation
    public let lifecycle: LifecyclePreview
    public let impact: [LoadoutImpact]
    public let diagnostic: LoadoutDiagnostic?

    public var applicable: Bool {
        lifecycle.applicable
    }

    public var scopeNotice: String {
        "Other profiles and projects using this source may also receive its content."
    }
}

public struct LoadoutResult: Sendable, Hashable {
    public let location: LoadoutLocation
    public let lifecycle: InstallationResult

    public var syncRequired: Bool {
        lifecycle.status != .failed
    }
}

public func discoverLoadout(
    home: ResourcePath, configHome: ResourcePath? = nil, environment: [String: String] = [:],
) throws -> LoadoutDiscovery {
    _ = try loadoutAbsolute(home)
    let base: ResourcePath = if let configHome {
        configHome
    } else if let configured = environment["XDG_CONFIG_HOME"], !configured.isEmpty {
        try ResourcePath(configured)
    } else {
        try appendPath(home, ".config")
    }
    let registry = try appendPath(loadoutAbsolute(base), "loadout/config.toml")
    do {
        guard let setup = try readLoadout(home: home, registry: registry) else {
            return .absent(registry: registry)
        }
        return .available(setup)
    } catch let error as LoadoutDiagnostic {
        return .invalid(registry: registry, diagnostic: error)
    } catch let error as TargetError {
        let code = error.code == .concurrentChange ? "concurrent-change" : "invalid-source"
        return .invalid(registry: registry, diagnostic: LoadoutDiagnostic(code, error.message, error.path ?? registry))
    }
}

private func readLoadout(home: ResourcePath, registry: ResourcePath) throws -> LoadoutSetup? {
    var inputs = LoadoutInputs()
    guard let data = try inputs.table(registry, code: "invalid-registry") else { return nil }
    let invalid = LoadoutDiagnostic(
        "invalid-registry",
        "Registration needs a valid source and optional profile",
        registry,
    )
    guard Set(data.keys).isSubset(of: ["source", "profile", "harnesses"]),
          let raw = loadoutText(data["source"]),
          let path = try? loadoutSourcePath(raw, home: home)
    else { throw invalid }
    let profile = loadoutText(data["profile"]) ?? "default"
    guard data["profile"] == nil || loadoutText(data["profile"]) == profile && !profile.isEmpty else { throw invalid }
    if let raw = data["harnesses"] {
        let defaults = try loadoutStrings(raw, registry, "harnesses", code: "invalid-registry")
        guard !defaults.isEmpty, Set(defaults).count == defaults.count,
              defaults.allSatisfy({ loadoutAgent($0) != nil })
        else {
            throw LoadoutDiagnostic("invalid-registry", "Registration harnesses must be unique known names", registry)
        }
    }
    let root = try inputs.directory(path)
    let manifest = try LoadoutManifest.read(root: root, profile: profile, home: home, inputs: &inputs)
    let conditions = inputs.conditions
    try conditions.forEach { try $0.require() }
    return LoadoutSetup(registry: registry, root: root, profile: profile, home: home,
                        manifest: manifest, conditions: conditions)
}

package func loadoutSyncNotice(_ bundleID: String) throws -> Notice {
    try Notice(artifactID: bundleID, kind: .activation,
               message: "Run Loadout sync to reflect these source changes. An update or removal can retire " +
                   "assets still referenced by generated hooks and MCP servers.", code: "loadout-sync-required")
}

public func previewLoadout(
    bundle: FlyrailBundle, destination: LoadoutDestination, store: any StateStore, options: LoadoutOptions,
) throws -> LoadoutPreview {
    let location = destination.location
    let target = try location.installation(store: store)
    var rendering = LoadoutSourceRender(bundle: bundle, destination: destination, options: options)
    let id = bundle.identity.id
    do {
        try destination.setup.conditions.forEach { try $0.require() }
        let rendered = try rendering.render(prefix: loadoutScopePrefix(target))
        let conditions = destination.setup.conditions + rendering.inputs.conditions
        try conditions.forEach { try $0.require() }
        let planned = try preview(bundle: bundle, rendered: rendered, target: target,
                                  replaceModified: options.replaceModified)
        let ownership = try loadoutOwnershipConflict(planned)
        let guarded = try LifecyclePreview(
            bundleID: id, bundle: bundle, rendered: rendered, target: target,
            acquisition: planned.acquisition, replaceModified: planned.replaceModified, index: planned.index,
            generation: planned.generation, resources: planned.resources,
            error: ownership.map { TargetError(code: .conflict, message: $0.message, path: $0.path) } ?? planned.error,
            artifactsByResource: planned.artifactsByResource, loadoutReads: conditions,
        )
        return LoadoutPreview(location: location, lifecycle: guarded, impact: rendering.impacts, diagnostic: ownership)
    } catch {
        let issue: LoadoutDiagnostic
        let failure: TargetError
        if let diagnostic = error as? LoadoutDiagnostic {
            issue = diagnostic
            failure = TargetError(code: .unsupported, message: issue.message, path: issue.path)
        } else if let problem = error as? TargetError, problem.code == .concurrentChange {
            issue = LoadoutDiagnostic("concurrent-change", problem.message, problem.path ?? destination.setup.registry)
            failure = problem
        } else {
            issue = LoadoutDiagnostic("source-plan", String(describing: error), destination.source.path)
            failure = TargetError(code: .invalidState, message: issue.message, path: issue.path)
        }
        let lifecycle = try LifecyclePreview(
            bundleID: id, bundle: bundle, rendered: .empty, target: target, acquisition: .conflict,
            replaceModified: options.replaceModified, index: nil, generation: nil, resources: [], error: failure,
        )
        return LoadoutPreview(location: location, lifecycle: lifecycle, impact: rendering.impacts, diagnostic: issue)
    }
}

private func loadoutScopePrefix(_ target: InstallationTarget) throws -> String {
    try String(target.contextDigest.prefix(32)) + "-"
}

private func loadoutOwnershipConflict(_ planned: LifecyclePreview) throws -> LoadoutDiagnostic? {
    let prefix = try loadoutScopePrefix(planned.target)
    return planned.resources.first { resource in
        resource.previous?.claims.contains(where: {
            exactTextEqual($0.bundleID, planned.bundleID) && $0.artifactID.hasPrefix(prefix) == false
        }) == true
    }.map {
        LoadoutDiagnostic(
            "source-ownership",
            "This bundle already owns the resource through another installation scope",
            $0.resource.destination,
        )
    }
}

public func previewLoadoutRemoval(
    bundleID: String, location: LoadoutLocation, store: any StateStore, replaceModified: Bool = false,
) throws -> LoadoutPreview {
    let rendered = try RenderedBundle(artifacts: [], notices: [loadoutSyncNotice(bundleID)])
    let planned = try lifecyclePreview(
        bundleID: bundleID, bundle: nil, rendered: rendered, target: location.installation(store: store),
        acquisition: .conflict, replaceModified: replaceModified,
    )
    let ownership = try loadoutOwnershipConflict(planned)
    let guarded = try LifecyclePreview(
        bundleID: bundleID, bundle: nil, rendered: rendered, target: planned.target,
        acquisition: planned.acquisition, replaceModified: planned.replaceModified, index: planned.index,
        generation: planned.generation, resources: planned.resources,
        error: ownership.map { TargetError(code: .conflict, message: $0.message, path: $0.path) } ?? planned.error,
    )
    return LoadoutPreview(location: location, lifecycle: guarded, impact: [], diagnostic: ownership)
}

public func applyLoadout(_ plan: LoadoutPreview, lockTimeout: Duration = .zero) -> LoadoutResult {
    let prepared = plan.lifecycle
    let result: InstallationResult
    if prepared.applicable {
        result = applyPreview(prepared, lockTimeout: lockTimeout)
    } else {
        let problem = prepared.error ?? prepared.resources.compactMap(\.error).first ??
            TargetError(code: .recoveryNeeded, message: "Recover the Loadout source installation and preview again")
        result = InstallationInspection.failed(prepared.bundleID, target: prepared.target, error: problem)
    }
    return LoadoutResult(location: plan.location, lifecycle: result)
}

public func inspectLoadout(
    bundleID: String, location: LoadoutLocation, store: any StateStore,
) throws -> InstallationObservation {
    try inspectInstallation(bundleID: bundleID, target: location.installation(store: store))
}

public func recoverLoadout(
    bundleID: String, location: LoadoutLocation, store: any StateStore, lockTimeout: Duration = .zero,
) throws -> LoadoutResult {
    try LoadoutResult(location: location, lifecycle: recoverInstallation(
        bundleID: bundleID, target: location.installation(store: store), lockTimeout: lockTimeout,
    ))
}
