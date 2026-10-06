import AppKit
import XCTest
@testable import DesktopInteraction

final class MacClipboardTests: XCTestCase {
    @MainActor func testWorkerCallbackWritesNativePasteboardWithoutEchoing() async throws {
        // A private pasteboard preserves the user's actual copied contents.
        let pasteboard = NSPasteboard.withUniqueName()
        defer { pasteboard.releaseGlobally() }
        let writer = MacClipboard(pasteboard: pasteboard)
        try await Task.detached { try writer.setText(text: "remote fixture 📝") }.value
        XCTAssertEqual(pasteboard.string(forType: .string), "remote fixture 📝")
        XCTAssertNil(writer.localChange())
        pasteboard.clearContents()
        XCTAssertTrue(pasteboard.setString("local fixture", forType: .string))
        XCTAssertEqual(writer.localChange(), "local fixture")
        XCTAssertNil(writer.localChange())
        try await Task.detached { try writer.setText(text: "") }.value
        XCTAssertEqual(pasteboard.string(forType: .string), "")
        XCTAssertNil(writer.localChange())
    }
}
