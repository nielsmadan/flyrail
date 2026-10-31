@testable import Flyrail
import Foundation

func stateRelocatedFixture(_ value: JSONValue) throws -> JSONValue {
    switch value {
    case let .array(items):
        return try .array(items.map(stateRelocatedFixture))
    case let .object(items):
        if items.count == 1, items[0].0 == "path", case let .string(original) = items[0].1 {
            let basename = String(original.split(separator: "/").last ?? "")
            let path = try ResourcePath("/pin/\(basename)")
            if basename.hasPrefix(".flyrail-") {
                let transactionID = String(basename.dropLast(".txn".count).suffix(32))
                return try StateCodec.path(transactionPath(
                    destination: ResourcePath(original.contains("config.json") ? "/pin/config.json" : "/pin/AGENTS.md"),
                    transactionID: transactionID,
                ))
            }
            return try StateCodec.path(path)
        }
        let relocated = try JSONValue.object(items.map { try ($0.0, stateRelocatedFixture($0.1)) })
        switch stateRecordTag(relocated) {
        case "Receipt":
            let fields = try stateField(relocated, "fields")
            let reference = try ResourceRef.fromState(stateField(fields, "resource"))
            let claims = try stateArray(stateField(fields, "claims")).map { claim -> JSONValue in
                let fields = try stateField(claim, "fields")
                let identity = try Claim.fromState(stateField(fields, "claim"))
                    .identity(destination: reference.destination)
                return try stateReplacingField(
                    claim,
                    "fields",
                    stateReplacingField(fields, "claim_id", .string(identity)),
                )
            }
            return try stateReplacingField(relocated, "fields", stateReplacingField(fields, "claims", .array(claims)))
        default:
            return relocated
        }
    default:
        return value
    }
}

private func stateReplacingField(_ value: JSONValue, _ key: String, _ replacement: JSONValue) throws -> JSONValue {
    guard case let .object(items) = value else { throw FlyrailValueError("invalid fixture object") }
    return .object(items.map { ($0.0, $0.0 == key ? replacement : $0.1) })
}
