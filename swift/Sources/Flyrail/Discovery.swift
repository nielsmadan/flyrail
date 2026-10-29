import Darwin
import Foundation

public struct AgentPresence: Sendable, Hashable {
    public let agent: Agent
    public let executable: ResourcePath?
    public let application: ResourcePath?

    public init(agent: Agent, executable: ResourcePath? = nil, application: ResourcePath? = nil) throws {
        guard executable != nil || application != nil else {
            throw FlyrailValueError("agent presence requires at least one piece of evidence")
        }
        for path in [executable, application].compactMap(\.self) where path.platformBytes.first != 47 {
            throw FlyrailValueError("agent presence evidence must be an absolute Path")
        }
        self.agent = agent
        self.executable = executable
        self.application = application
    }
}

public func detectAgents(path: [ResourcePath]? = nil, applications: [ResourcePath]? = nil) throws -> [AgentPresence] {
    let directories: [ResourcePath]
    if let path { directories = path } else {
        let value = ProcessInfo.processInfo.environment["PATH"] ?? ""
        directories = try value.isEmpty ? [] :
            value.split(separator: ":", omittingEmptySubsequences: false)
            .map { try ResourcePath($0.isEmpty ? "." : String($0)) }
    }
    let applications = try applications ?? [
        ResourcePath("/Applications"),
        ResourcePath(FileManager.default.homeDirectoryForCurrentUser.appending(path: "Applications").path),
    ]
    return try Agent.allCases.compactMap { agent in
        let executable = try directories.lazy.compactMap { directory -> ResourcePath? in
            let candidate = try appendPath(directory, agent.rawValue)
            return discoveryEvidence(candidate, directory: false) ? try discoveryAbsolute(candidate) : nil
        }.first
        let name: String? = switch agent {
        case .cursor: "Cursor.app"
        case .copilot: "Visual Studio Code.app"
        case .claude, .codex, .opencode, .pi, .droid: nil
        }
        let application = try name.flatMap { name in
            try applications.lazy.compactMap { directory -> ResourcePath? in
                let candidate = try appendPath(directory, name)
                return discoveryEvidence(candidate, directory: true) ? try discoveryAbsolute(candidate) : nil
            }.first
        }
        guard executable != nil || application != nil else { return nil }
        return try AgentPresence(agent: agent, executable: executable, application: application)
    }
}

private func discoveryEvidence(_ path: ResourcePath, directory: Bool) -> Bool {
    path.filePath.withPlatformString { pointer in
        var metadata = stat()
        guard fstatat(AT_FDCWD, pointer, &metadata, 0) == 0 else { return false }
        return directory ? metadata.st_mode & S_IFMT == S_IFDIR :
            metadata.st_mode & S_IFMT == S_IFREG && access(pointer, X_OK) == 0
    }
}

private func discoveryAbsolute(_ path: ResourcePath) throws -> ResourcePath {
    let absolute = try NativeAuthority.absolute(path)
    let parts = absolute.platformBytes.split(separator: 47)
    var normalized: [[UInt8]] = []
    for part in parts {
        if part.elementsEqual([46]) { continue }
        if part.elementsEqual([46, 46]) {
            if !normalized.isEmpty { normalized.removeLast() }
        } else { normalized.append(Array(part)) }
    }
    return try ResourcePath(platformBytes: [47] + normalized.joined(separator: [47]))
}
