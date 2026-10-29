// swift-tools-version: 6.2
import PackageDescription

let package = Package(
    name: "Flyrail",
    platforms: [
        .macOS(.v13),
    ],
    products: [
        .library(name: "Flyrail", targets: ["Flyrail"]),
    ],
    dependencies: [
        .package(url: "https://github.com/apple/swift-system", exact: "1.8.1"),
        .package(url: "https://github.com/swiftlang/swift-docc-plugin", exact: "1.5.0"),
    ],
    targets: [
        .target(
            name: "Flyrail",
            dependencies: [
                .product(name: "SystemPackage", package: "swift-system"),
            ],
            path: "swift/Sources/Flyrail",
            resources: [
                .copy("Resources"),
            ]
        ),
        .target(
            name: "FlyrailInteropSupport",
            dependencies: ["Flyrail"],
            path: "swift/Sources/FlyrailInteropSupport"
        ),
        .executableTarget(
            name: "FlyrailInterop",
            dependencies: ["FlyrailInteropSupport"],
            path: "swift/Sources/FlyrailInterop"
        ),
        .testTarget(
            name: "FlyrailTests",
            dependencies: ["Flyrail", "FlyrailInteropSupport"],
            path: "swift/Tests/FlyrailTests",
            resources: [
                .copy("Fixtures"),
            ]
        ),
    ],
    swiftLanguageModes: [.v6]
)
