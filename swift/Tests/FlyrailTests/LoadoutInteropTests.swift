import Flyrail
import Foundation
import Testing

@Test(.enabled(if: ProcessInfo.processInfo.environment["FLYRAIL_LOADOUT_INTEROP_ROOT"] != nil,
               "run spec/check_loadout_interop.py to provision the fixture"))
func loadoutInteropProducer() throws {
    let environment = ProcessInfo.processInfo.environment
    let root = try URL(fileURLWithPath: #require(environment["FLYRAIL_LOADOUT_INTEROP_ROOT"]), isDirectory: true)
    func child(_ name: String) throws -> ResourcePath {
        try ResourcePath(root.appending(path: name).path)
    }
    let store = try DirectoryStore(child("state"))
    let phase = try #require(environment["FLYRAIL_LOADOUT_INTEROP_PHASE"])
    if phase == "remove" {
        let source = try ResourcePath(#require(environment["FLYRAIL_LOADOUT_INTEROP_SOURCE"]))
        let location = try LoadoutLocation(root: source, profile: "default", sourceName: "local", sourcePath: source)
        for bundleID in ["extra", "interop"] {
            #expect(try inspectLoadout(bundleID: bundleID, location: location, store: store).isCurrent)
            let plan = try previewLoadoutRemoval(bundleID: bundleID, location: location, store: store)
            try #require(plan.applicable, Comment(rawValue: String(describing: plan.lifecycle.error)))
            let result = applyLoadout(plan)
            #expect(result.lifecycle.status == .applied, Comment(rawValue: String(describing: result.lifecycle.error)))
            #expect(result.syncRequired)
        }
        return
    }

    try #require(phase == "install")
    let node = try ResourcePath(#require(environment["FLYRAIL_LOADOUT_INTEROP_NODE"]))
    let runtime = try HookRuntime(node: node, shell: .posix)
    let options = try LoadoutOptions(platform: .macOS, hookRuntime: runtime, executionRoot: ResourcePath(root.path))
    for bundleID in ["interop", "extra"] {
        let bundle = try FlyrailBundle(directory: child(bundleID))
        var agents: [Agent] = [.claude, .codex, .cursor, .pi]
        if bundleID == "interop" {
            agents.append(.opencode)
        }
        let statuses: [OperationStatus] = [.applied, .unchanged]
        for expected in statuses {
            let discovered = try discoverLoadout(home: child("home"))
            guard case let .available(setup) = discovered else {
                Issue.record("Missing fixture setup: \(discovered)")
                return
            }
            let destination = try setup.select(source: "local", harnesses: agents)
            let preview = try previewLoadout(bundle: bundle, destination: destination, store: store, options: options)
            try #require(
                preview.applicable,
                Comment(
                    rawValue: "diagnostic: \(String(describing: preview.diagnostic)), error: \(String(describing: preview.lifecycle.error))",
                ),
            )
            let result = applyLoadout(preview)
            #expect(result.lifecycle.status == expected, Comment(rawValue: String(describing: result.lifecycle.error)))
            if expected == .applied {
                #expect(result.syncRequired)
            }
            #expect(try inspectLoadout(bundleID: bundleID, location: preview.location, store: store).isCurrent)
        }
    }
}
