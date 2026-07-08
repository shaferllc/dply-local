// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "DplyLocal",
    platforms: [.macOS(.v14)],
    targets: [
        .executableTarget(
            name: "DplyLocal",
            path: "Sources/DplyLocal",
            // Stay in Swift 5 language mode: this app is a thin GUI over the
            // `dpl` CLI and doesn't need strict-concurrency ceremony.
            swiftSettings: [.swiftLanguageMode(.v5)]
        )
    ]
)
