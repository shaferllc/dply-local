// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "DplyLocal",
    platforms: [.macOS(.v14)],
    dependencies: [
        // Self-updates: the app checks the GitHub Releases appcast and updates
        // in place (EdDSA-signed archives; see make-app.sh + release.yml).
        .package(url: "https://github.com/sparkle-project/Sparkle", from: "2.9.0"),
    ],
    targets: [
        .executableTarget(
            name: "DplyLocal",
            dependencies: [
                .product(name: "Sparkle", package: "Sparkle"),
            ],
            path: "Sources/DplyLocal",
            // Stay in Swift 5 language mode: this app is a thin GUI over the
            // `dpl` CLI and doesn't need strict-concurrency ceremony.
            swiftSettings: [.swiftLanguageMode(.v5)],
            linkerSettings: [
                // Inside the .app bundle, Sparkle.framework lives in
                // Contents/Frameworks (make-app.sh puts it there). SwiftPM's own
                // rpath for the artifacts dir still applies for `swift run`.
                .unsafeFlags(["-Xlinker", "-rpath", "-Xlinker", "@executable_path/../Frameworks"]),
            ]
        )
    ]
)
