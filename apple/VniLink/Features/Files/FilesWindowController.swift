import AppKit
import SwiftUI
import VnilinkCore

@MainActor final class FilesWindowController: NSWindowController, NSWindowDelegate {
    private final class WeakWindow { weak var value: FilesWindowController?; init(_ value: FilesWindowController) { self.value = value } }
    private static var windows: [String: WeakWindow] = [:]
    static func open(core: VnilinkCore, session: @MainActor @escaping () -> FfiSessionInfo?, peerId: String, localName: String, remoteName: String) -> FilesWindowController {
        windows = windows.filter { $0.value.value != nil }
        let key = "\(core.endpointId()):\(peerId)"
        if let existing = windows[key]?.value, existing.window?.isVisible == true {
            existing.window?.makeKeyAndOrderFront(nil)
            return existing
        }
        let controller = FilesWindowController(core: core, session: session, peerId: peerId, localName: localName, remoteName: remoteName)
        windows[key] = WeakWindow(controller)
        return controller
    }
    private let model: FilesModel
    private init(core: VnilinkCore, session: @MainActor @escaping () -> FfiSessionInfo?, peerId: String, localName: String, remoteName: String) {
        model = FilesModel(backend: ServiceFilesBackend(core: core, peerId: peerId), session: session, peerId: peerId, localName: localName, remoteName: remoteName)
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 980, height: 640),
                              styleMask: [.titled, .closable, .miniaturizable, .resizable], backing: .buffered, defer: false)
        window.title = "Files — \(remoteName)"
        window.minSize = NSSize(width: 720, height: 480)
        window.contentView = NSHostingView(rootView: FilesView(model: model))
        window.isReleasedWhenClosed = false
        super.init(window: window)
        window.delegate = self
        window.center()
    }
    func selectUpload(_ url: URL) { model.selectUpload(url) }
    required init?(coder: NSCoder) { fatalError("init(coder:) is unavailable") }
    func windowWillClose(_ notification: Notification) { model.stop() }
}
