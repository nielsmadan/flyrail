public extension Key {
    static func == (left: Self, right: Self) -> Bool {
        exactModelEqual(left, right)
    }

    func hash(into hasher: inout Hasher) {
        hashExactModel(self, into: &hasher)
    }
}

public extension Member {
    static func == (left: Self, right: Self) -> Bool {
        exactModelEqual(left, right)
    }

    func hash(into hasher: inout Hasher) {
        hashExactModel(self, into: &hasher)
    }
}

public extension SelectorPart {
    static func == (left: Self, right: Self) -> Bool {
        exactModelEqual(left, right)
    }

    func hash(into hasher: inout Hasher) {
        hashExactModel(self, into: &hasher)
    }
}

public extension Selector {
    static func == (left: Self, right: Self) -> Bool {
        exactModelEqual(left, right)
    }

    func hash(into hasher: inout Hasher) {
        hashExactModel(self, into: &hasher)
    }
}

public extension Placement {
    static func == (left: Self, right: Self) -> Bool {
        exactModelEqual(left, right)
    }

    func hash(into hasher: inout Hasher) {
        hashExactModel(self, into: &hasher)
    }
}

public extension FileContent {
    static func == (left: Self, right: Self) -> Bool {
        exactModelEqual(left, right)
    }

    func hash(into hasher: inout Hasher) {
        hashExactModel(self, into: &hasher)
    }
}

public extension SectionBoundaries {
    static func == (left: Self, right: Self) -> Bool {
        exactModelEqual(left, right)
    }

    func hash(into hasher: inout Hasher) {
        hashExactModel(self, into: &hasher)
    }
}

public extension SectionContent {
    static func == (left: Self, right: Self) -> Bool {
        exactModelEqual(left, right)
    }

    func hash(into hasher: inout Hasher) {
        hashExactModel(self, into: &hasher)
    }
}

public extension StructuredContent {
    static func == (left: Self, right: Self) -> Bool {
        exactModelEqual(left, right)
    }

    func hash(into hasher: inout Hasher) {
        hashExactModel(self, into: &hasher)
    }
}

public extension DocumentSchema {
    static func == (left: Self, right: Self) -> Bool {
        exactModelEqual(left, right)
    }

    func hash(into hasher: inout Hasher) {
        hashExactModel(self, into: &hasher)
    }
}

public extension Content {
    static func == (left: Self, right: Self) -> Bool {
        exactModelEqual(left, right)
    }

    func hash(into hasher: inout Hasher) {
        hashExactModel(self, into: &hasher)
    }
}

public extension Audience {
    static func == (left: Self, right: Self) -> Bool {
        exactModelEqual(left, right)
    }

    func hash(into hasher: inout Hasher) {
        hashExactModel(self, into: &hasher)
    }
}

public extension EnvRef {
    static func == (left: Self, right: Self) -> Bool {
        exactModelEqual(left, right)
    }

    func hash(into hasher: inout Hasher) {
        hashExactModel(self, into: &hasher)
    }
}

public extension AssetRef {
    static func == (left: Self, right: Self) -> Bool {
        exactModelEqual(left, right)
    }

    func hash(into hasher: inout Hasher) {
        hashExactModel(self, into: &hasher)
    }
}

public extension CommandArgument {
    static func == (left: Self, right: Self) -> Bool {
        exactModelEqual(left, right)
    }

    func hash(into hasher: inout Hasher) {
        hashExactModel(self, into: &hasher)
    }
}

public extension EnvironmentValue {
    static func == (left: Self, right: Self) -> Bool {
        exactModelEqual(left, right)
    }

    func hash(into hasher: inout Hasher) {
        hashExactModel(self, into: &hasher)
    }
}

public extension Command {
    static func == (left: Self, right: Self) -> Bool {
        exactModelEqual(left, right)
    }

    func hash(into hasher: inout Hasher) {
        hashExactModel(self, into: &hasher)
    }
}

public extension HttpTransport {
    static func == (left: Self, right: Self) -> Bool {
        exactModelEqual(left, right)
    }

    func hash(into hasher: inout Hasher) {
        hashExactModel(self, into: &hasher)
    }
}

public extension MCPTransport {
    static func == (left: Self, right: Self) -> Bool {
        exactModelEqual(left, right)
    }

    func hash(into hasher: inout Hasher) {
        hashExactModel(self, into: &hasher)
    }
}

public extension InstructionArtifact {
    static func == (left: Self, right: Self) -> Bool {
        exactModelEqual(left, right)
    }

    func hash(into hasher: inout Hasher) {
        hashExactModel(self, into: &hasher)
    }
}

public extension MCPArtifact {
    static func == (left: Self, right: Self) -> Bool {
        exactModelEqual(left, right)
    }

    func hash(into hasher: inout Hasher) {
        hashExactModel(self, into: &hasher)
    }
}

public extension HookArtifact {
    static func == (left: Self, right: Self) -> Bool {
        exactModelEqual(left, right)
    }

    func hash(into hasher: inout Hasher) {
        hashExactModel(self, into: &hasher)
    }
}

public extension NativeArtifact {
    static func == (left: Self, right: Self) -> Bool {
        exactModelEqual(left, right)
    }

    func hash(into hasher: inout Hasher) {
        hashExactModel(self, into: &hasher)
    }
}

public extension SupportAsset {
    static func == (left: Self, right: Self) -> Bool {
        exactModelEqual(left, right)
    }

    func hash(into hasher: inout Hasher) {
        hashExactModel(self, into: &hasher)
    }
}

public extension Dependency {
    static func == (left: Self, right: Self) -> Bool {
        exactModelEqual(left, right)
    }

    func hash(into hasher: inout Hasher) {
        hashExactModel(self, into: &hasher)
    }
}

public extension Artifact {
    static func == (left: Self, right: Self) -> Bool {
        exactModelEqual(left, right)
    }

    func hash(into hasher: inout Hasher) {
        hashExactModel(self, into: &hasher)
    }
}

private protocol ExactModel {
    func modelValue() throws -> SemanticValue
}

extension Key: ExactModel {}
extension Member: ExactModel {}
extension SelectorPart: ExactModel {}
extension Selector: ExactModel {}
extension Placement: ExactModel {}
extension FileContent: ExactModel {}
extension SectionBoundaries: ExactModel {}
extension SectionContent: ExactModel {}
extension StructuredContent: ExactModel {}
extension DocumentSchema: ExactModel {}
extension Content: ExactModel {}
extension Audience: ExactModel {}
extension EnvRef: ExactModel {}
extension AssetRef: ExactModel {}
extension CommandArgument: ExactModel {}
extension EnvironmentValue: ExactModel {}
extension Command: ExactModel {}
extension HttpTransport: ExactModel {}
extension MCPTransport: ExactModel {}
extension InstructionArtifact: ExactModel {}
extension MCPArtifact: ExactModel {}
extension HookArtifact: ExactModel {}
extension NativeArtifact: ExactModel {}
extension SupportAsset: ExactModel {}
extension Dependency: ExactModel {}
extension Artifact: ExactModel {}

private func exactModelEqual(_ left: some ExactModel, _ right: some ExactModel) -> Bool {
    guard let leftValue = try? left.modelValue(), let rightValue = try? right.modelValue() else {
        return false
    }
    return leftValue == rightValue
}

private func hashExactModel(_ value: some ExactModel, into hasher: inout Hasher) {
    hasher.combine(try? value.modelValue())
}
