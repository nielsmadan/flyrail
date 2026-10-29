import Flyrail
import Foundation

package enum InteropDifferential {
    static func run(_ arguments: [String]) throws -> Data {
        let (corpusPath, rootPath) = try options(arguments)
        let input = try FileHandle(forReadingFrom: URL(fileURLWithPath: corpusPath))
        defer { try? input.close() }
        let bytes = try boundedInteropInput(input, limit: 16 * 1024 * 1024,
                                            message: "invalid differential corpus")
        guard let text = String(data: bytes, encoding: .utf8) else {
            throw FlyrailValueError("invalid differential corpus")
        }
        let corpus = try Corpus(text: text)
        let root = URL(fileURLWithPath: rootPath).standardizedFileURL
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: false)
        for row in corpus.renders {
            try FileManager.default.createDirectory(at: root.appending(path: row.name),
                                                    withIntermediateDirectories: false)
        }
        let records = try valueRecords(corpus.values) + bundleRecords(corpus.bundles) +
            corpus.renders.flatMap { try renderRecords($0, root: root) } + corpus.edits.flatMap(editRecords) +
            lifecycleRecords(corpus.lifecycles, root: root) + lifecycleEdgeRecords(root: root)
        return Data(records.joined(separator: "\n").utf8) + Data([10])
    }

    private static func options(_ arguments: [String]) throws -> (corpus: String, root: String) {
        let usage = FlyrailValueError("usage: differential --corpus FILE --root DIRECTORY")
        guard arguments.count.isMultiple(of: 2) else { throw usage }
        var values: [String: String] = [:]
        for index in stride(from: 0, to: arguments.count, by: 2) {
            let name = arguments[index]
            guard ["--corpus", "--root"].contains(name), values.updateValue(arguments[index + 1], forKey: name) == nil
            else { throw usage }
        }
        guard let corpus = values["--corpus"], let root = values["--root"] else { throw usage }
        return (corpus, root)
    }

    static func record(_ category: String, _ name: String, _ field: String, _ data: Data) -> String {
        "\(category)\t\(name)\t\(field)\t\(data.nativeHex)"
    }

    package static func valueRecords(_ rows: [ValueRow]) throws -> [String] {
        try rows
            .map { try record("value", $0.name, "semantic", SemanticValue(semanticBytes: $0.bytes).semanticBytes()) }
    }

    package static func bundleRecords(_ rows: [BundleRow]) throws -> [String] {
        try rows.flatMap { row in
            let bundle = try FlyrailBundle(manifest: row.manifest, entries: row.entries)
            return try [record("bundle", row.name, "digest", Data(bundle.contentDigest.utf8)),
                        record("bundle", row.name, "canonical", bundle.differentialCanonicalValue().semanticBytes())]
        }
    }

    private static func renderRecords(_ row: RenderRow, root: URL) throws -> [String] {
        let caseRoot = root.appending(path: row.name)
        let suffix = row.name.split(separator: "-").last.map(String.init) ?? ""
        let first = try RenderedArtifact(
            id: "base-\(suffix)", family: .instructions,
            destination: ResourcePath(caseRoot.appending(path: row.filename).path),
            content: .file(FileContent(data: row.data, mode: .readable))
        )
        let second = try RenderedArtifact(
            id: "section-\(suffix)", family: .instructions,
            destination: ResourcePath(caseRoot.appending(path: "AGENTS.md").path),
            content: .section(SectionContent(marker: "section-\(suffix)", text: "value-\(row.value)\n"))
        )
        let context = [("alpha", "Straße"), ("zeta", row.value)]
        let rendered = try RenderedBundle(
            artifacts: row.fileFirst ? [first, second] : [second, first],
            dependencies: [Dependency(dependent: second.id, required: first.id)],
            routingContext: row.alphaFirst ? context : context.reversed()
        )
        let canonical = try SemanticValue.array([
            .array(rendered.artifacts.map { try $0.modelValue() }),
            .array(rendered.dependencies.map { try $0.modelValue() }),
        ]).semanticBytes()
        let routes = try SemanticValue.array(rendered.routingContext.map {
            .array([.scalar(.text($0.0)), .scalar(.text($0.1))])
        }).semanticBytes()
        let claims = try rendered.artifacts.enumerated().map { index, artifact in
            let claim = try artifact.id == first
                .id ? Claim(kind: .file, selector: .whole) : contentClaim(artifact.content)
            return try record("render", row.name, "claim-\(index)",
                              Data(claim.identity(destination: artifact.destination).utf8))
        }
        return [record("render", row.name, "digest", Data(rendered.renderDigest.utf8)),
                record("render", row.name, "canonical", canonical),
                record("render", row.name, "context", routes)] + claims
    }

    private static func editContent(_ format: EditFormat, text: String) throws -> Content {
        switch format {
        case .section: try .section(SectionContent(marker: "managed", text: text))
        case let .structured(format):
            try .structured(StructuredContent(format: format, selector: Selector(parts: [.key(Key("tool"))]),
                                              value: .scalar(.text(text))))
        }
    }

    private static func editRecords(_ row: EditRow) throws -> [String] {
        var data = row.source
        var ownership: [OwnedSelection] = []
        var provenance = DocumentProvenance()
        var claim: Claim?
        var records: [String] = []
        for (step, operation) in row.operations.enumerated() {
            let request: DocumentRequest
            switch operation {
            case let .set(text, acquisition):
                request = try .set(editContent(row.format, text: text), acquisition: acquisition)
                claim = request.claim
            case .retire:
                guard let claim else { throw FlyrailValueError("invalid differential edit operation") }
                request = try .retire(claim)
            }
            let edit = try editDocument(source: data, requests: [request], ownership: ownership, provenance: provenance)
            data = edit.after
            ownership = edit.ownership
            provenance = edit.provenance
            let prefix = "\(step + 1)-"
            records += try [
                record("edit", row.name, prefix + "bytes", data ?? Data("-".utf8)),
                record("edit", row.name, prefix + "status", Data(edit.observations.map(\.status.rawValue)
                        .joined(separator: ",").utf8)),
                record("edit", row.name, prefix + "provenance", StateCodec.encode(provenance)),
                record("edit", row.name, prefix + "ownership-count", Data(String(ownership.count).utf8)),
            ]
            records += try ownership.enumerated().map { ownedIndex, owned in
                try record("edit", row.name, prefix + "ownership-\(ownedIndex)", StateCodec.encode(owned))
            }
        }
        return records
    }
}
