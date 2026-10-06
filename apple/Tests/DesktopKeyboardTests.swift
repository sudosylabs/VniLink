import AppKit
import XCTest
@testable import DesktopInteraction

final class DesktopKeyboardTests: XCTestCase {
    func testUntrackedRightModifierReleaseDoesNotCreateAPhantomPress() {
        var keys = DesktopKeyboard()
        _ = keys.modifier(code: 55, flags: .command)
        XCTAssertEqual(keys.modifier(code: 54, flags: []), DesktopKey(route: .local, usage: 227, down: false, text: ""))
        XCTAssertTrue(keys.held.isEmpty)
    }
    func testFocusTimeModifierSnapshotPreservesRightLeftTransitions() {
        var keys = DesktopKeyboard()
        XCTAssertEqual(keys.synchronize(.command), [DesktopKey(route: .local, usage: 227, down: true, text: "")])
        XCTAssertTrue(keys.synchronize(.command).isEmpty)
        XCTAssertEqual(keys.modifier(code: 54, flags: []), DesktopKey(route: .local, usage: 227, down: false, text: ""))
        XCTAssertTrue(keys.held.isEmpty)
        _ = keys.modifier(code: 56, flags: .shift)
        _ = keys.modifier(code: 60, flags: .shift)
        XCTAssertTrue(keys.synchronize(.shift).isEmpty)
        XCTAssertEqual(keys.synchronize([]).map(\.usage), [225, 229])
        XCTAssertTrue(keys.held.isEmpty)
    }
    func testNavigationFunctionAndControlCharactersStayPhysicalKeys() {
        var keys = DesktopKeyboard()
        XCTAssertEqual(keys.key(code: 123, down: true, text: "\u{f702}")?.text, "")
        XCTAssertEqual(keys.key(code: 122, down: true, text: "\u{f704}")?.text, "")
        XCTAssertEqual(keys.key(code: 49, down: true, text: "\0")?.text, "")
        XCTAssertEqual(keys.key(code: 36, down: true, text: "\r")?.text, "")
        XCTAssertEqual(keys.key(code: 0, down: true, text: "é")?.text, "é")
    }
    func testShortcutModifiersAndFocusReleasesUseTheSamePlannerAsText() {
        var keys = DesktopKeyboard()
        XCTAssertEqual(keys.modifier(code: 55, flags: .command), DesktopKey(route: .local, usage: 227, down: true, text: ""))
        XCTAssertEqual(keys.key(code: 8, down: true, text: "c"), DesktopKey(route: .local, usage: 6, down: true, text: "c"))
        XCTAssertEqual(keys.release(), [DesktopKey(route: .local, usage: 6, down: false, text: ""), DesktopKey(route: .local, usage: 227, down: false, text: "")])
        XCTAssertTrue(keys.held.isEmpty)
        XCTAssertNil(keys.key(code: 8, down: false))
    }
    func testShiftOptionAndRepeatKeepTextAndReleaseState() {
        var keys = DesktopKeyboard()
        XCTAssertEqual(keys.modifier(code: 56, flags: .shift)?.route, .local)
        XCTAssertEqual(keys.key(code: 0, down: true, text: "A")?.text, "A")
        XCTAssertEqual(keys.key(code: 0, down: true, text: "A")?.text, "A")
        XCTAssertEqual(keys.key(code: 0, down: false)?.text, "")
        XCTAssertEqual(keys.modifier(code: 56, flags: [])?.down, false)
        XCTAssertTrue(keys.held.isEmpty)
    }
}
