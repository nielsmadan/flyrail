import Foundation

private let posixFileTypeMask: UInt32 = 0o170000
private let posixRegularFile: UInt32 = 0o100000
private let posixDirectory: UInt32 = 0o040000
private let windowsReparsePoint: UInt32 = 0x400

package struct ZIPBundleSource: BundleSource {
    fileprivate enum EntryReference {
        case directory
        case file(Int)
    }

    fileprivate struct EntryNode {
        var reference: EntryReference?
        var children: [Data: Int] = [:]
    }

    private struct TraversalStep {
        let component: Data
        let node: Int
        let parentPathLength: Int
    }

    private struct PortableNode {
        var children: [String: (spelling: String, node: Int)] = [:]
    }

    private let archive: ZIPArchive
    private let root: String
    private let nodes: [EntryNode]
    private let rootNode: Int

    package init(archive: ZIPArchive, root rootValue: String) throws {
        let root = rootValue.hasSuffix("/") ? String(rootValue.dropLast()) : rootValue
        let rootComponents = pathComponents(root)
        let rootKeys = rootComponents.map(portablePathKey)
        var nodes = [EntryNode()]
        var portableNodes = [PortableNode()]
        var explicit: Set<[UInt8]> = []
        for (index, member) in archive.members.enumerated() {
            let name = member.name.hasSuffix("/") ? String(member.name.dropLast()) : member.name
            let components = pathComponents(name)
            let keys = components.map(portablePathKey)
            let selected = overlaps(keys, rootKeys)
            guard selected else {
                continue
            }
            var portableNode = 0
            for (component, key) in zip(components, keys) {
                try validateRelativePath(component, label: "archive path")
                if let existing = portableNodes[portableNode].children[key] {
                    guard exactTextEqual(existing.spelling, component) else {
                        throw FlyrailValueError("portable archive name collision: \(name)")
                    }
                    portableNode = existing.node
                } else {
                    let child = portableNodes.count
                    portableNodes.append(PortableNode())
                    portableNodes[portableNode].children[key] = (component, child)
                    portableNode = child
                }
            }
            guard !member.originalName.contains("\0") else {
                throw FlyrailValueError("ambiguous archive entry: \(pythonRepr(member.originalName))")
            }
            let original = member.originalName.hasSuffix("/")
                ? String(member.originalName.dropLast())
                : member.originalName
            try validateRelativePath(original, label: "archive path")
            guard exactTextEqual(member.originalName, member.name) else {
                throw FlyrailValueError("ambiguous archive entry: \(pythonRepr(member.originalName))")
            }
            let expected = name + (member.isDirectory ? "/" : "")
            guard exactTextEqual(member.name, expected),
                  explicit.insert(Array(name.utf8)).inserted
            else {
                throw FlyrailValueError("duplicate or ambiguous archive entry: \(member.name)")
            }
            let mode = member.externalAttributes >> 16
            let kind = member.createSystem == 3 ? mode & posixFileTypeMask : 0
            let expectedKind = member.isDirectory ? posixDirectory : posixRegularFile
            guard kind == 0 || kind == expectedKind else {
                throw FlyrailValueError("archive symlinks and special files are not supported: \(name)")
            }
            guard member.externalAttributes & windowsReparsePoint == 0 else {
                throw FlyrailValueError("archive reparse points are not supported: \(name)")
            }
            var node = 0
            for (componentIndex, component) in components.enumerated() {
                let bytes = Data(component.utf8)
                if let child = nodes[node].children[bytes] {
                    node = child
                } else {
                    let child = nodes.count
                    nodes.append(EntryNode())
                    nodes[node].children[bytes] = child
                    node = child
                }
                let isLeaf = componentIndex == components.count - 1
                guard !isLeaf || nodes[node].reference == nil else {
                    throw FlyrailValueError("archive file/directory collision: \(name)")
                }
                if isLeaf {
                    guard member.isDirectory || nodes[node].children.isEmpty else {
                        throw FlyrailValueError("archive file/directory collision: \(name)")
                    }
                    nodes[node].reference = member.isDirectory ? .directory : .file(index)
                } else if case .file? = nodes[node].reference {
                    throw FlyrailValueError("archive file/directory collision: \(name)")
                }
            }
        }
        guard let rootNode = findNode(rootComponents, in: nodes),
              !isFile(nodes[rootNode].reference)
        else {
            throw FlyrailValueError("package resource must be an existing directory")
        }
        self.archive = archive
        self.root = root
        self.nodes = nodes
        self.rootNode = rootNode
    }

    package func manifest() throws -> Data {
        let path = ["flyrail.json"]
        guard let node = findNode(path, from: rootNode, in: nodes),
              case let .file(index)? = nodes[node].reference
        else {
            throw FlyrailValueError("package resource must contain a flyrail.json file")
        }
        return try archive.data(at: index, limits: .default)
    }

    package func tree(spec: SkillSpec, budget: inout BundleEntryBudget) throws -> [BundleEntry] {
        try sourceTree(
            path: spec.path,
            installedRoot: spec.name,
            executables: spec.executables,
            budget: &budget
        )
    }

    private func sourceTree(
        path sourcePath: String,
        installedRoot: String,
        executables: [String],
        budget: inout BundleEntryBudget
    ) throws -> [BundleEntry] {
        let sourceComponents = pathComponents(sourcePath)
        guard let sourceRoot = findNode(sourceComponents, from: rootNode, in: nodes),
              !isFile(nodes[sourceRoot].reference)
        else {
            throw FlyrailValueError("skill source must be an existing directory: \(sourcePath)")
        }
        let installedEntry = try BundleEntry(path: installedRoot)
        try budget.reserve(path: installedEntry.path, dataBytes: nil)
        var result = [installedEntry]
        var path = Data(installedRoot.utf8)
        let relativeStart = path.count + 1
        let initial = nodes[sourceRoot].children
            .map { (component: $0.key, node: $0.value) }
            .sorted { $1.component.lexicographicallyPrecedes($0.component) }
        var stack = initial.map {
            TraversalStep(
                component: $0.component,
                node: $0.node,
                parentPathLength: path.count
            )
        }
        while let current = stack.popLast() {
            if path.count > current.parentPathLength {
                path.removeLast(path.count - current.parentPathLength)
            }
            path.append(UInt8(ascii: "/"))
            path.append(current.component)
            let installedPath = String(decoding: path, as: UTF8.self)
            let reference = nodes[current.node].reference
            if case let .file(index)? = reference {
                let size = try archive.uncompressedSize(at: index)
                try budget.reserve(path: installedPath, dataBytes: size)
                let data = try archive.data(at: index, limits: .default)
                try result.append(BundleEntry(
                    path: installedPath,
                    data: data,
                    executable: executables.contains {
                        $0.utf8.elementsEqual(path.dropFirst(relativeStart))
                    }
                ))
            } else {
                try budget.reserve(path: installedPath, dataBytes: nil)
                try result.append(BundleEntry(path: installedPath))
            }
            let children = nodes[current.node].children
                .map { (component: $0.key, node: $0.value) }
                .sorted { $1.component.lexicographicallyPrecedes($0.component) }
            stack.append(contentsOf: children.map {
                TraversalStep(
                    component: $0.component,
                    node: $0.node,
                    parentPathLength: path.count
                )
            })
        }
        return result
    }

    package func readFile(path: String) throws -> Data {
        guard let node = findNode(pathComponents(path), from: rootNode, in: nodes),
              case let .file(index)? = nodes[node].reference
        else {
            throw FlyrailValueError("expected a source file: \(path)")
        }
        return try archive.data(at: index, limits: .default)
    }

    package func readTree(path: String, executables: [String]) throws -> [BundleEntry] {
        var budget = BundleEntryBudget(limits: .default)
        let entries = try sourceTree(
            path: path,
            installedRoot: "content",
            executables: executables,
            budget: &budget
        )
        let prefix = "content/"
        return try entries.compactMap { entry in
            guard !exactTextEqual(entry.path, "content") else {
                return nil
            }
            return try BundleEntry(
                path: String(entry.path.dropFirst(prefix.count)),
                data: entry.data,
                executable: entry.executable
            )
        }
    }
}

private func pathComponents(_ path: String) -> [String] {
    path.split(separator: "/", omittingEmptySubsequences: false).map(String.init)
}

private func overlaps(_ left: [String], _ right: [String]) -> Bool {
    zip(left, right).allSatisfy { exactTextEqual($0, $1) }
}

private func findNode(
    _ components: [String],
    from start: Int = 0,
    in nodes: [ZIPBundleSource.EntryNode]
) -> Int? {
    var node = start
    for component in components {
        guard let child = nodes[node].children[Data(component.utf8)] else {
            return nil
        }
        node = child
    }
    return node
}

private func isFile(_ reference: ZIPBundleSource.EntryReference?) -> Bool {
    if case .file? = reference {
        return true
    }
    return false
}

private func pythonRepr(_ value: String) -> String {
    var output = "'"
    for scalar in value.unicodeScalars {
        switch scalar.value {
        case 0:
            output += "\\x00"
        case 9:
            output += "\\t"
        case 10:
            output += "\\n"
        case 13:
            output += "\\r"
        case 39:
            output += "\\'"
        case 92:
            output += "\\\\"
        default:
            output.unicodeScalars.append(scalar)
        }
    }
    return output + "'"
}
