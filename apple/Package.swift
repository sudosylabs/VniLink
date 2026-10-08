// swift-tools-version: 5.9
import PackageDescription
let package = Package(name: "DesktopInteraction", platforms: [.macOS("15.0")], dependencies: [.package(path: "VnilinkCore")], targets: [
    .target(name: "DesktopInteraction", dependencies: [.product(name: "VnilinkCore", package: "VnilinkCore")], path: "VniLink/Features/Viewer", exclude: ["ViewerWindow.swift"], sources: ["DesktopKeyboard.swift", "DesktopFiles.swift", "MacClipboard.swift", "FilesModel.swift", "ServiceFilesBackend.swift"], linkerSettings: [.linkedFramework("Carbon"), .linkedFramework("Security"), .linkedFramework("SystemConfiguration"), .linkedLibrary("resolv")]),
    .target(name: "NativeSessionPages", path: "VniLink/App", exclude: ["AppModel.swift", "BackgroundSettings.swift", "MainWindow.swift", "SessionWorkspace.swift", "VniLinkApp.swift"], sources: ["SessionPages.swift"]),
    .testTarget(name: "DesktopInteractionTests", dependencies: ["DesktopInteraction", "NativeSessionPages"], path: "Tests")
])
