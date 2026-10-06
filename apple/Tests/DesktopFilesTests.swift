import Foundation
import XCTest
@testable import DesktopInteraction

final class DesktopFilesTests: XCTestCase {
    func testBoundedFileReadingIncludesEmptyFilesAndRejectsFoldersAndOversize() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let file = root.appendingPathComponent("fixture.bin")
        try Data().write(to: file)
        XCTAssertEqual(try DesktopFiles.read(file), Data())
        try Data([0, 255, 10]).write(to: file)
        XCTAssertEqual(try DesktopFiles.read(file), Data([0, 255, 10]))
        XCTAssertThrowsError(try DesktopFiles.read(root))
        try Data(count: DesktopFiles.limit + 1).write(to: file)
        XCTAssertThrowsError(try DesktopFiles.read(file))
    }
    func testRemoteClipboardWritesAreNotEchoedAndNewLocalCopyStillSends() {
        var state = ClipboardChanges(changeCount: 3)
        XCTAssertFalse(state.changed(3))
        XCTAssertTrue(state.changed(4))
        state.applied(6)
        XCTAssertFalse(state.changed(6))
        XCTAssertTrue(state.changed(7))
        XCTAssertFalse(state.changed(7))
    }
}
