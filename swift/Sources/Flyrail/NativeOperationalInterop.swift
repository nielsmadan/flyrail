import Darwin
import Foundation

extension NativeInterop {
    package static func operate(_ command: InteropCommand, options: [String: String], output: FileHandle) throws {
        guard let indexPath = options["--index"] else { throw FlyrailValueError("missing required argument: --index") }
        let target = try InstallationTarget(indexRoot: ResourcePath(indexPath))
        let owner = options["--bundle-id"] ?? "team"
        try validateIdentifier(owner, label: "bundle id")
        let timeout = try duration(options["--lock-timeout"] ?? "0")
        let seam = try seam(options)
        let prepared = try command == .write ? prepareWrite(options: options, owner: owner) : nil
        if command == .inspect {
            try output.write(contentsOf: StateCodec.encodeRecord(observationValue(
                inspectInstallation(bundleID: owner, target: target),
            )))
            return
        }
        let result: InstallationResult
        do {
            switch command {
            case .recover:
                let observed = try NativeAdmission.observeIndex(target, bundleID: owner)
                if observed.current == nil, observed.prepared == nil {
                    result = recoverInstallation(bundleID: owner, target: target, lockTimeout: timeout)
                } else {
                    let locks = try LifecycleExecution.locks(bundleID: owner, target: target, observed: observed,
                                                             resources: [], timeout: timeout)
                    result = try LifecycleExecution.recover(bundleID: owner, target: target, locks: locks, seam: seam)
                }
            case .remove:
                result = remove(bundleID: owner, target: target, replaceModified: options["--replace-modified"] != nil,
                                lockTimeout: timeout)
            case .write:
                guard let (bundle, rendered, acquisition) = prepared else {
                    throw FlyrailValueError("missing write arguments")
                }
                result = LifecycleExecution.synchronize(
                    bundleID: owner, bundle: bundle, rendered: rendered, target: target,
                    acquisition: acquisition, replaceModified: options["--replace-modified"] != nil,
                    lockTimeout: timeout, seam: seam,
                )
            default: throw FlyrailValueError("unsupported operational command")
            }
        } catch {
            let error = NativeTransactionExecution.detail(error)
            let observation = try inspectInstallation(bundleID: owner, target: target)
            result = InstallationResult(
                status: error.code == .recoveryNeeded ? .incomplete : .failed, observation: observation, error: error,
            )
        }
        try output.write(contentsOf: StateCodec.encodeRecord(resultValue(result)))
    }

    private static func prepareWrite(
        options: [String: String], owner: String,
    ) throws -> (FlyrailBundle, RenderedBundle, Acquisition) {
        guard let destination = options["--destination"],
              let acquisition = Acquisition(rawValue: options["--acquisition"] ?? "conflict")
        else { throw FlyrailValueError("invalid destination or acquisition") }
        guard let source = options["--source"] else {
            guard options["--agent"] == nil, options["--node"] == nil else {
                throw FlyrailValueError("--agent and --node require --source")
            }
            let bundle = try FlyrailBundle(
                identity: BundleIdentity(id: owner, version: options["--version"] ?? "1"),
                artifacts: [.instruction(InstructionArtifact(id: "guide", text: "text"))], assets: [],
                dependencies: [],
            )
            return try (bundle, rendered(options, destination: ResourcePath(destination)), acquisition)
        }
        if let option = ["--mode", "--large", "--kind", "--section", "--payload", "--version"]
            .first(where: { options[$0] != nil })
        {
            throw FlyrailValueError("\(option) cannot be combined with --source")
        }
        guard let agent = Agent(rawValue: options["--agent"] ?? "codex") else {
            throw FlyrailValueError("invalid agent")
        }
        let bundle = try FlyrailBundle(directory: ResourcePath(source))
        guard exactTextEqual(bundle.identity.id, owner) else {
            throw FlyrailValueError("source bundle id must match --bundle-id")
        }
        let context = try RenderContext(
            target: .project(agent: agent, root: NativeAuthority.absolute(ResourcePath(destination))),
            platform: .macOS, surface: .cli,
        ).hookRuntime(HookRuntime(node: options["--node"].map(ResourcePath.init)))
        return try (bundle, render(bundle, for: context), acquisition)
    }

    private static func duration(_ value: String) throws -> Duration {
        guard let seconds = Double(value), seconds.isFinite, seconds >= 0, seconds <= 1e9 else {
            throw FlyrailValueError("timeout must be finite and nonnegative")
        }
        return .seconds(seconds)
    }

    private static func rendered(_ options: [String: String], destination: ResourcePath) throws -> RenderedBundle {
        guard let mode = FileMode(rawValue: options["--mode"] ?? "readable") else {
            throw FlyrailValueError("invalid mode")
        }
        let payload: Data
        if let size = options["--large"] {
            guard let size = Int(size), (0 ... 1024 * 1024).contains(size) else {
                throw FlyrailValueError("large payload size must be between 0 and 1048576")
            }
            let pattern = Array("flyrail-interop-payload\n".utf8)
            payload = Data((0 ..< size).lazy.map { pattern[$0 % pattern.count] })
        } else {
            payload = Data((options["--payload"] ?? "generated\n").utf8)
        }
        guard payload.count <= 1024 * 1024 else {
            throw FlyrailValueError("payload must not exceed 1048576 bytes")
        }
        guard options["--section"] == nil || options["--kind"] != "tree" else {
            throw FlyrailValueError("tree resources cannot have a section")
        }
        let content: Content
        if let marker = options["--section"] {
            guard let text = String(data: payload, encoding: .utf8)
            else { throw FlyrailValueError("section needs text") }
            content = try .section(SectionContent(marker: marker, text: text))
        } else if options["--kind"] == "tree" {
            content = try .tree(TreeContent(entries: [
                BundleEntry(path: "empty"),
                BundleEntry(path: "nested/file", data: payload, executable: mode == .executable),
            ]))
        } else {
            guard options["--kind"] == nil || options["--kind"] == "document" else {
                throw FlyrailValueError("invalid resource kind")
            }
            content = .file(FileContent(data: payload, mode: mode))
        }
        return try RenderedBundle(artifacts: [
            RenderedArtifact(id: "guide", family: options["--kind"] == "tree" ? .skills : .instructions,
                             destination: NativeAuthority.absolute(destination), content: content),
        ])
    }

    private static func seam(_ options: [String: String]) throws -> NativeTransactionSeam {
        guard options["--fail-index-completion"] == nil || options["--fault-phase"] == nil else {
            throw FlyrailValueError("fault options cannot be combined")
        }
        let fault = options["--fail-index-completion"] != nil ? "index-completion" : options["--fault-phase"]
        let phase = options["--barrier-phase"]
        let ready = options["--barrier-ready"]
        let release = options["--barrier-release"]
        let boundaries: Set = [
            "stage-node", "index-pending-created", "journal-created", "receipt-created", "index-completion-created",
            "index-pending-prepared", "index-pending-reread", "index-pending-committed",
            "resource-precondition", "index-require", "index-ancestor", "guard-lookup",
            "journal-prepared", "journal-reread", "journal-committed", "prepare", "rename", "backup",
            "staging", "publish", "receipt", "receipt-prepared", "receipt-reread", "receipt-committed",
            "cleanup-node", "cleanup-metadata", "cleanup", "index-completion", "index-completion-prepared",
            "index-completion-reread", "index-completion-committed", "rollback-stage", "rollback-backup",
            "rollback", "recover-journal", "recover-index",
        ]
        guard [fault, phase].compactMap(\.self).allSatisfy(boundaries.contains) else {
            throw FlyrailValueError("unknown transaction seam")
        }
        guard fault == nil || (phase == nil && ready == nil && release == nil &&
            options["--barrier-timeout"] == nil)
        else { throw FlyrailValueError("fault and barrier options cannot be combined") }
        if let fault {
            return NativeTransactionSeam {
                if $0 == fault {
                    throw TargetError(code: .ioError, message: "injected \(fault) fault", errno: EIO)
                }
            }
        }
        guard let phase else {
            guard ready == nil, release == nil, options["--barrier-timeout"] == nil else {
                throw FlyrailValueError("barrier options require --barrier-phase")
            }
            return .none
        }
        guard let ready, let release else {
            throw FlyrailValueError("--barrier-phase requires --barrier-ready and --barrier-release")
        }
        let wait = try duration(options["--barrier-timeout"] ?? "30")
        let readyPath = try NativeAuthority.absolute(ResourcePath(ready))
        let releasePath = try NativeAuthority.absolute(ResourcePath(release))
        var visited = false
        return NativeTransactionSeam { boundary in
            guard boundary == phase, !visited else { return }
            visited = true
            try NativeTransactionIO.createFile(readyPath, data: Data("ready\n".utf8))
            let deadline = ContinuousClock.now.advanced(by: wait)
            while try NativeFilesystem.metadata(releasePath) == nil {
                guard ContinuousClock.now < deadline else { throw FlyrailValueError("interop barrier timed out") }
                Thread.sleep(forTimeInterval: 0.005)
            }
        }
    }

    private static func errorValue(_ error: TargetError?) -> JSONValue {
        error.map { .object([
            ("code", .string($0.code.rawValue)), ("message", .string($0.message)),
            ("path", $0.path.map { .string($0.displayString) } ?? .null),
        ]) } ?? .null
    }

    private static func observationValue(_ observation: InstallationObservation) -> JSONValue {
        .object([
            ("bundle_id", .string(observation.bundleID)), ("is_current", .boolean(observation.isCurrent)),
            ("pending", .boolean(observation.pending)), ("version", observation.version.map(JSONValue.string) ?? .null),
            ("error", errorValue(observation.error)),
            ("resources", .array(observation.resources.map { resource in
                .object([
                    ("destination", .string(resource.destination.displayString)),
                    ("state_root", .string(resource.stateRoot.displayString)),
                    ("claims", .array(resource.claims.map {
                        .object([("artifact_id", .string($0.artifactID)), ("disposition", .string($0.status.rawValue))])
                    })),
                    ("error", errorValue(resource.error)),
                    ("recovery_paths", .array(resource.recoveryPaths.map { .string($0.displayString) })),
                ])
            })),
        ])
    }

    private static func resultValue(_ result: InstallationResult) -> JSONValue {
        .object([
            ("status", .string(result.status.rawValue)), ("error", errorValue(result.error)),
            ("observation", observationValue(result.observation)),
            ("resources", .array(result.resources.map {
                .object([
                    ("destination", .string($0.resource.destination.displayString)),
                    ("status", .string($0.status.rawValue)),
                    ("error", errorValue($0.error)), ("recovery_paths", .array($0.recoveryPaths.map {
                        .string($0.displayString)
                    })),
                ])
            })),
        ])
    }
}
