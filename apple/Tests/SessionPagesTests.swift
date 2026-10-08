import AppKit
import XCTest
@testable import NativeSessionPages

final class SessionPagesTests: XCTestCase {
    func testSwitchingRetainsIndependentPageControllers() async {
        await MainActor.run {
            _ = NSApplication.shared
            let tabs = SessionPagesController()
            let home = NSViewController(), a = NSViewController(), b = NSViewController()
            home.view = NSView(); a.view = NSView(); b.view = NSView()
            var created = [String: Int]()
            let pages = [("home", home), ("a", a), ("b", b)].map { id, controller in
                SessionPagesController.Page(id: id, title: id) {
                    created[id, default: 0] += 1
                    return controller
                }
            }
            tabs.synchronize(pages, selected: "a")
            XCTAssertEqual(tabs.selectedTabViewItemIndex, 1)
            for id in ["b", "home", "a", "b"] { tabs.synchronize(pages, selected: id) }
            XCTAssertEqual(created, ["home": 1, "a": 1, "b": 1])
            XCTAssertTrue(tabs.tabViewItems[1].viewController === a)
            XCTAssertTrue(tabs.tabViewItems[2].viewController === b)
            XCTAssertEqual(tabs.selectedTabViewItemIndex, 2)
            XCTAssertEqual(tabs.children.count, 3)
        }
    }
    func testClosingOnePageKeepsHomeAndOtherController() async {
        await MainActor.run {
            _ = NSApplication.shared
            let tabs = SessionPagesController()
            let home = NSViewController(), a = NSViewController(), b = NSViewController()
            home.view = NSView(); a.view = NSView(); b.view = NSView()
            let homePage = SessionPagesController.Page(id: "home", title: "Home") { home }
            let aPage = SessionPagesController.Page(id: "a", title: "Computer A") { a }
            let bPage = SessionPagesController.Page(id: "b", title: "Computer B") { b }
            tabs.synchronize([homePage, aPage, bPage], selected: "a")
            tabs.synchronize([homePage, bPage], selected: "b")
            XCTAssertEqual(tabs.tabViewItems.map { $0.identifier as? String }, ["home", "b"])
            XCTAssertFalse(tabs.children.contains { $0 === a })
            XCTAssertTrue(tabs.children.contains { $0 === b })
            XCTAssertTrue(tabs.tabViewItems[0].viewController === home)
            XCTAssertEqual(tabs.selectedTabViewItemIndex, 1)
            tabs.synchronize([homePage], selected: "home")
            XCTAssertEqual(tabs.tabViewItems.count, 1)
            XCTAssertTrue(tabs.tabViewItems[0].viewController === home)
        }
    }
}
