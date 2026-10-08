import AppKit

/// AppKit owns persistent child controllers; selection only changes visibility.
/// The Home page is never removed. Session resources belong to their page owner.
@MainActor final class SessionPagesController: NSTabViewController {
    struct Page {
        let id: String
        let title: String
        let makeController: () -> NSViewController
    }
    override func loadView() {
        super.loadView()
        tabStyle = .unspecified
        canPropagateSelectedChildViewControllerTitle = false
        tabView.tabViewType = .noTabsNoBorder
        transitionOptions = []
    }
    func synchronize(_ pages: [Page], selected: String) {
        _ = view
        let ids = Set(pages.map(\.id))
        for item in tabViewItems where !ids.contains(item.identifier as? String ?? "") {
            removeTabViewItem(item)
        }
        for page in pages {
            if let item = tabViewItems.first(where: { $0.identifier as? String == page.id }) {
                item.label = page.title
            } else {
                let item = NSTabViewItem(viewController: page.makeController())
                item.identifier = page.id; item.label = page.title
                addTabViewItem(item)
            }
        }
        if let index = tabViewItems.firstIndex(where: { $0.identifier as? String == selected }), selectedTabViewItemIndex != index {
            selectedTabViewItemIndex = index
        }
    }
}
