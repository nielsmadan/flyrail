import Foundation
import Testing

private let fixtureCaseCounts = [
    "bundles": ["cases": 3],
    "configurations": [
        "cases": 4, "semantic_values": 10, "claims": 17, "claim_overlaps": 11,
        "authorities": 6, "renderings": 3, "receipts": 6, "journals": 2,
        "transactions": 9, "capabilities": 7, "preparations": 1,
    ],
    "edits": ["cases": 27, "invalid": 6],
    "hooks": ["cases": 58],
    "interop-source-options": ["cases": 9],
    "translations": ["cases": 35, "combined_cases": 4],
]

private let unnamedFixtureCategories: Set = [
    "configurations.semantic_values", "configurations.claim_overlaps", "interop-source-options.cases",
]

@Test
func sharedFixtureCategoriesCannotSilentlyDisappear() throws {
    let keys = fixtureCaseCounts.flatMap { name, categories in categories.keys.map { "\(name).\($0)" } }
    #expect(unnamedFixtureCategories.isSubset(of: keys))
    for (name, categories) in fixtureCaseCounts {
        let url = try #require(Bundle.module.url(forResource: name, withExtension: "json", subdirectory: "Fixtures"))
        let document = try #require(JSONSerialization.jsonObject(with: Data(contentsOf: url)) as? [String: Any])
        #expect(Set(document.filter { $0.value is [Any] }.keys) == Set(categories.keys), "\(name)")
        for (category, count) in categories {
            let key = "\(name).\(category)"
            let cases = try #require(document[category] as? [Any], "\(key)")
            #expect(cases.count == count, "\(key)")
            let names = cases.map { ($0 as? [String: Any])?["name"] as? String }
            if unnamedFixtureCategories.contains(key) {
                #expect(names.allSatisfy { $0 == nil }, "\(key) is listed as unnamed")
            } else {
                let present = try names.map { try #require($0, "\(key) case without a name") }
                #expect(Set(present.map { Data($0.utf8) }).count == present.count, "\(key) names repeat")
            }
        }
    }
}
