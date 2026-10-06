// swift-tools-version: 5.9
import PackageDescription
let package = Package(name: "DesktopInteraction", platforms: [.macOS("15.0")], dependencies: [.package(path: "VnilinkCore")], targets: [
    .target(name: "DesktopInteraction", dependencies: [.product(name: "VnilinkCore", package: "VnilinkCore")], path: "VniLink/Features/Viewer", exclude: ["ViewerWindow.swift"], sources: ["DesktopKeyboard.swift", "DesktopFiles.swift", "MacClipboard.swift"], linkerSettings: [.linkedFramework("Carbon"), .linkedFramework("Security"), .linkedFramework("SystemConfiguration"), .linkedLibrary("resolv")]),
    .testTarget(name: "DesktopInteractionTests", dependencies: ["DesktopInteraction"], path: "Tests")
])
