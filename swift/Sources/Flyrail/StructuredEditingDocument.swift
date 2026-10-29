import Foundation

package protocol StructuredEditingDocument: AnyObject {
    var data: Data { get }
    func native(_ path: [DocumentPathPart]) throws -> SemanticValue
    func syntax(_ path: [DocumentPathPart]) throws -> Data
    func ownedSyntax(_ path: [DocumentPathPart]) throws -> Data
    func layout(_ path: [DocumentPathPart]) throws -> Data
    func emptySyntax(_ path: [DocumentPathPart]) throws -> Data
    func positionPrefix(_ path: [DocumentPathPart]) throws -> Data
    func put(_ path: [DocumentPathPart], value: SemanticValue, syntax: Data?, layout: Data) throws
    // swiftlint:disable:next function_parameter_count
    func insert(
        _ path: [DocumentPathPart],
        index: Int,
        value: SemanticValue,
        syntax: Data?,
        prefix: Data?,
        layout: Data,
    ) throws
    func delete(_ path: [DocumentPathPart]) throws
    // swiftlint:disable:next function_parameter_count
    func relocate(
        _ path: [DocumentPathPart],
        index: Int,
        value: SemanticValue,
        syntax: Data?,
        prefix: Data?,
        layout: Data,
    ) throws
}

package extension StructuredEditingDocument {
    func put(_ path: [DocumentPathPart], value: SemanticValue, syntax: Data? = nil, layout: Data = Data()) throws {
        try put(path, value: value, syntax: syntax, layout: layout)
    }

    func insert(
        _ path: [DocumentPathPart], index: Int, value: SemanticValue,
        syntax: Data? = nil, prefix: Data? = nil, layout: Data = Data(),
    ) throws {
        try insert(path, index: index, value: value, syntax: syntax, prefix: prefix, layout: layout)
    }

    func relocate(
        _ path: [DocumentPathPart], index: Int, value: SemanticValue,
        syntax: Data? = nil, prefix: Data? = nil, layout: Data = Data(),
    ) throws {
        try relocate(path, index: index, value: value, syntax: syntax, prefix: prefix, layout: layout)
    }
}

extension JSONEditingDocument: StructuredEditingDocument {
    package func ownedSyntax(_ path: [DocumentPathPart]) throws -> Data {
        try syntax(path)
    }

    package func emptySyntax(_ path: [DocumentPathPart]) throws -> Data {
        try syntax(path)
    }

    package func layout(_: [DocumentPathPart]) -> Data {
        Data()
    }

    package func put(_ path: [DocumentPathPart], value: SemanticValue, syntax: Data?, layout _: Data) throws {
        try put(path, value: value, syntax: syntax)
    }

    // swiftlint:disable:next function_parameter_count
    package func insert(
        _ path: [DocumentPathPart], index: Int, value: SemanticValue,
        syntax: Data?, prefix: Data?, layout _: Data,
    ) throws {
        try insert(path, index: index, value: value, syntax: syntax, prefix: prefix)
    }

    // swiftlint:disable:next function_parameter_count
    package func relocate(
        _ path: [DocumentPathPart], index: Int, value: SemanticValue,
        syntax: Data?, prefix: Data?, layout _: Data,
    ) throws {
        try relocate(path, index: index, value: value, syntax: syntax, prefix: prefix)
    }
}

package func editingBackend(data: Data, format: DocumentFormat) throws -> any StructuredEditingDocument {
    if format == .toml {
        return try TOMLEditingDocument(data: data)
    }
    return try JSONEditingDocument(data: data, comments: format == .jsonc)
}
