import Flyrail
import Foundation

enum InteropHookRendering {
    static func run(_ input: Data) throws -> Data {
        guard let request = try JSONSerialization.jsonObject(with: input) as? [String: Any],
              let manifest = request["manifest"] as? [String: Any],
              let root = request["root"] as? String,
              let agentName = request["agent"] as? String, let agent = Agent(rawValue: agentName),
              let scope = request["scope"] as? String, ["project", "user"].contains(scope),
              let platformName = request["platform"] as? String, let platform = Platform(rawValue: platformName),
              let surfaceName = request["surface"] as? String, let surface = Surface(rawValue: surfaceName)
        else { throw FlyrailValueError("invalid hook rendering request") }
        let bundle = try FlyrailBundle(manifest: JSONSerialization.data(withJSONObject: manifest), entries: [])
        let target = try scope == "project"
            ? Target.project(agent: agent, root: ResourcePath(root))
            : Target.user(agent: agent, home: ResourcePath(root))
        var context = try RenderContext(target: target, platform: platform, surface: surface)
        if let assetRoot = request["asset_root"] as? String {
            context = try context.assetRoot(ResourcePath(assetRoot))
        }
        let locations: [(String, (RenderContext, ResourcePath) throws -> RenderContext)] = [
            ("execution_root", { try $0.executionRoot($1) }), ("instruction_path", { try $0.instructionPath($1) }),
            ("mcp_path", { try $0.mcpPath($1) }), ("native_root", { try $0.nativeRoot($1) }),
        ]
        for (key, apply) in locations {
            switch request[key] {
            case nil, is NSNull: continue
            case let path as String: context = try apply(context, ResourcePath(path))
            default: throw FlyrailValueError("invalid hook rendering \(key)")
            }
        }
        if request["runtime"] as? Bool == true {
            let shell = try (request["shell"] as? String).map { name in
                guard let shell = HookShell(rawValue: name) else { throw FlyrailValueError("invalid hook shell") }
                return shell
            }
            context = try context.hookRuntime(HookRuntime(
                node: (request["node"] as? String).map(ResourcePath.init), shell: shell
            ))
        }
        let rendered = try render(bundle, for: context)
        return try SemanticValue.array([
            .array(rendered.artifacts.map { try $0.modelValue() }),
            .array(rendered.dependencies.map { try $0.modelValue() }),
            .array(rendered.notices.map {
                .array([.scalar(.text($0.artifactID)), .scalar(.text($0.kind.rawValue)),
                        .scalar(.text($0.message)), .scalar(.text($0.code))])
            }),
            .array(rendered.routingContext.map { .array([.scalar(.text($0.0)), .scalar(.text($0.1))]) }),
            .scalar(.text(rendered.renderDigest)),
        ]).semanticBytes()
    }
}
