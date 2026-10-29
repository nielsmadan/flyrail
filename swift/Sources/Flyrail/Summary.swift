import Foundation

public enum ChangeAction: String, Sendable, Hashable, CaseIterable {
    case create, update, delete, unchanged, conflict
}

public struct PlannedChange: Sendable, Hashable {
    public let artifactIDs: [String]
    public let families: [Family]
    public let kind: ResourceKind
    public let destination: ResourcePath
    public let action: ChangeAction
    public let error: TargetError?
    public let recoveryPaths: [ResourcePath]

    public init(artifactIDs: [String], families: [Family], kind: ResourceKind, destination: ResourcePath,
                action: ChangeAction, error: TargetError? = nil, recoveryPaths: [ResourcePath] = []) throws
    {
        guard artifactIDs.count == families.count else {
            throw FlyrailValueError("planned change artifact ids and families must agree")
        }
        try requireResourcePath(destination)
        try recoveryPaths.forEach(requireResourcePath)
        self.artifactIDs = artifactIDs
        self.families = families
        self.kind = kind
        self.destination = destination
        self.action = action
        self.error = error
        self.recoveryPaths = recoveryPaths
    }

    public static func == (left: Self, right: Self) -> Bool {
        left.artifactIDs.map { Data($0.utf8) } == right.artifactIDs.map { Data($0.utf8) } &&
            left.families == right.families && left.kind == right.kind && left.destination == right.destination &&
            left.action == right.action && left.error == right.error && left.recoveryPaths == right.recoveryPaths
    }

    public func hash(into hasher: inout Hasher) {
        hasher.combine(artifactIDs.map { Data($0.utf8) })
        hasher.combine(families)
        hasher.combine(kind)
        hasher.combine(destination)
        hasher.combine(action)
        hasher.combine(error)
        hasher.combine(recoveryPaths)
    }
}

public struct PlanSummary: Sendable, Hashable {
    public let agent: Agent?
    public let scope: TargetScope?
    public let surface: Surface?
    public let applicable: Bool
    public let changes: [PlannedChange]
    public let notices: [Notice]
    public let error: TargetError?

    public init(agent: Agent? = nil, scope: TargetScope? = nil, surface: Surface? = nil, applicable: Bool,
                changes: [PlannedChange], notices: [Notice] = [], error: TargetError? = nil)
    {
        self.agent = agent
        self.scope = scope
        self.surface = surface
        self.applicable = applicable
        self.changes = changes
        self.notices = notices
        self.error = error
    }
}

public func summarize(_ preview: LifecyclePreview) throws -> PlanSummary {
    func routing(_ key: String) -> String? {
        preview.target.routingContext.first { exactTextEqual($0.0, key) }?.1
    }
    let changes = try preview.resources.map { plan in
        let matched = (preview.artifactsByResource[plan.resource] ?? [])
            .sorted { utf8Less($0.id, $1.id) }
        return try PlannedChange(artifactIDs: matched.map(\.id), families: matched.map(\.family),
                                 kind: plan.resource.kind, destination: plan.resource.destination,
                                 action: changeAction(plan), error: plan.error, recoveryPaths: plan.recoveryPaths)
    }
    return try PlanSummary(agent: summaryRouting(routing("agent"), as: Agent.self),
                           scope: summaryRouting(routing("scope"), as: TargetScope.self),
                           surface: summaryRouting(routing("surface"), as: Surface.self),
                           applicable: preview.applicable,
                           changes: changes, notices: preview.rendered.notices, error: preview.error)
}

private func summaryRouting<T: RawRepresentable>(_ value: String?, as type: T.Type) throws -> T?
    where T.RawValue == String
{
    guard let value, !value.isEmpty else { return nil }
    guard let parsed = type.init(rawValue: value) else {
        throw FlyrailValueError("'\(value)' is not a valid \(type)")
    }
    return parsed
}

private func changeAction(_ plan: ResourcePlan) -> ChangeAction {
    if plan.error != nil || !plan.recoveryPaths.isEmpty { return .conflict }
    if plan.previous == plan.receipt, plan.before == plan.after { return .unchanged }
    if plan.after.nodes.isEmpty { return .delete }
    return plan.before.nodes.isEmpty ? .create : .update
}
