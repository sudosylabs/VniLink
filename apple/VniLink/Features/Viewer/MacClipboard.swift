import AppKit
import VnilinkCore

/// Core invokes this from a worker and waits for the native write to succeed.
/// This callback never calls the core, and its contents are never logged.
// All mutable state and pasteboard access is confined to the main thread;
// the Sendable callback explicitly hops there from core workers.
final class MacClipboard: ClipboardWriter, @unchecked Sendable {
    static let shared = MacClipboard()
    private let pasteboard: NSPasteboard
    private var changes: ClipboardChanges
    init(pasteboard: NSPasteboard = .general) {
        precondition(Thread.isMainThread)
        self.pasteboard = pasteboard
        changes = ClipboardChanges(changeCount: pasteboard.changeCount)
    }
    func setText(text: String) throws {
        let write = {
            self.pasteboard.clearContents()
            guard self.pasteboard.setString(text, forType: .string) else {
                throw FfiError.Unsupported(message: "The Mac clipboard could not be updated")
            }
            self.changes.applied(self.pasteboard.changeCount)
        }
        if Thread.isMainThread { try write() }
        else { try DispatchQueue.main.sync(execute: write) }
    }
    /// Main-thread only. Read only new local copies, never remote writes.
    func localChange() -> String? {
        precondition(Thread.isMainThread)
        guard changes.changed(pasteboard.changeCount) else { return nil }
        return pasteboard.string(forType: .string)
    }
    func baseline() { precondition(Thread.isMainThread); changes.applied(pasteboard.changeCount) }
}
