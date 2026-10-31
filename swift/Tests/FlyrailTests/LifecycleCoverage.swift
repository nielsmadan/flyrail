@testable import Flyrail
import Foundation
import Testing

func requireLifecycleCoverage(_ output: String, corpus: URL) throws {
    let rows = try String(contentsOf: corpus, encoding: .utf8).split(separator: "\n")
        .map { $0.split(separator: "\t").map(String.init) }.filter { $0.first == "lifecycle" }
    let records = output.split(separator: "\n").map { $0.split(separator: "\t").map(String.init) }
        .filter { $0.first == "lifecycle" }
    let keys = records.map { $0.prefix(3).joined(separator: "\t") }
    #expect(Set(keys).count == keys.count)
    #expect(rows.isEmpty == false)
    for row in rows {
        let name = row[1]
        for step in ["1-create", "2-unchanged", "3-update", "4-recover", "6-remove"] {
            for field in ["result", "payloads"] {
                #expect(keys.contains("lifecycle\t\(name)\t\(step)-\(field)"))
            }
            #expect(records.contains { $0[1] == name && $0[2].hasPrefix(step + "-state-") })
        }
        #expect(keys.contains("lifecycle\t\(name)\t5-inspect"))
    }
    for field in ["desired-generation", "stale-preview", "partial-inspect",
                  "source-free-recover-result", "partial-sync-result", "source-free-remove-result"]
    {
        #expect(keys.contains("lifecycle\tpublic-boundaries\t\(field)"))
    }
}
