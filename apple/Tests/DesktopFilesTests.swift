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
    func testSelectionUsesApprovedRootBoundariesAndDoesNotReadPayload() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let nested = root.appendingPathComponent("approved/nested")
        try FileManager.default.createDirectory(at: nested, withIntermediateDirectories: true)
        let file = nested.appendingPathComponent("large.bin")
        XCTAssertTrue(FileManager.default.createFile(atPath: file.path, contents: nil))
        let handle = try FileHandle(forWritingTo: file)
        try handle.truncate(atOffset: UInt64(DesktopFiles.limit + 1)); try handle.close()
        let approved = root.appendingPathComponent("approved")
        XCTAssertEqual(try FilesSelection.locate(file, roots: [(4, approved)]), FilesSelection(root: 4, path: ["nested"], name: "large.bin"))
        XCTAssertEqual(try FilesSelection.locate(file, roots: [(4, approved), (8, nested)]), FilesSelection(root: 8, path: [], name: "large.bin"))
        XCTAssertEqual(try FilesSelection.locate(nested, roots: [(4, approved)]).name, "nested")
        XCTAssertThrowsError(try FilesSelection.locate(approved, roots: [(4, approved)]))
        let outside = root.appendingPathComponent("approved-sibling")
        try Data().write(to: outside)
        XCTAssertThrowsError(try FilesSelection.locate(outside, roots: [(4, approved)]))
        let link = nested.appendingPathComponent("link")
        try FileManager.default.createSymbolicLink(at: link, withDestinationURL: file)
        XCTAssertThrowsError(try FilesSelection.locate(link, roots: [(4, approved)]))
    }

}
