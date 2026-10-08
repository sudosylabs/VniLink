import SwiftUI
import AppKit

/// A compact tab strip above persistent AppKit pages in the single main window.
struct SessionWorkspace: View {
    @EnvironmentObject var model: AppModel
    private var viewer: ViewerModel? { model.viewerTabs.first { $0.id == model.selectedTab } }
    var body: some View {
        VStack(spacing: 0) {
            ScrollView(.horizontal) {
                HStack(spacing: 4) {
                    tab("Home", id: nil)
                    ForEach(model.viewerTabs, id: \.id) { tab($0.name, id: $0.id) }
                }.padding(.horizontal, 10).padding(.vertical, 6)
            }
            .scrollIndicators(.hidden)
            .background(.bar)
            Divider()
            SessionPages(model: model)
        }
        .navigationTitle(viewer?.name ?? "VniLink")
        .navigationSubtitle(viewer == nil ? model.destination.rawValue : "Remote desktop")
        .toolbar {
            if let viewer {
                ToolbarItemGroup {
                    ViewerActions(model: viewer)
                }
            } else {
                ToolbarItem(placement: .navigation) {
                    Button { NotificationCenter.default.post(name: .toggleHomeSidebar, object: nil) } label: {
                        Label("Toggle sidebar", systemImage: "sidebar.left")
                    }
                }
                ToolbarItem(placement: .primaryAction) {
                    Menu {
                        Button("Add computer…") { model.adding = true }
                        Button("Create pairing file…") { model.createPairingFile() }
                        Button("Copy invitation") { model.copyInvitation() }
                    } label: { Label("Add computer", systemImage: "plus") }
                    .help("Add computer").disabled(model.snapshot == nil)
                }
            }
        }
    }
    private func tab(_ title: String, id: UUID?) -> some View {
        HStack(spacing: 0) {
            Button { model.selectedTab = id } label: {
                Text(title).lineLimit(1).truncationMode(.middle)
                    .font(.system(size: 13, weight: model.selectedTab == id ? .medium : .regular))
                    .padding(.leading, 12).padding(.trailing, id == nil ? 12 : 8).padding(.vertical, 8)
            }
            .buttonStyle(.plain)
            .accessibilityLabel(title)
            .accessibilityAddTraits(model.selectedTab == id ? .isSelected : [])
            if let id {
                Button { model.closeTab(id) } label: {
                    Image(systemName: "xmark").font(.system(size: 10, weight: .semibold))
                        .frame(width: 26, height: 28).contentShape(Rectangle())
                }
                .buttonStyle(.plain).help("Close \(title)")
                .accessibilityLabel("Close \(title)")
            }
        }
        .background(model.selectedTab == id ? Color(nsColor: .controlBackgroundColor) : .clear, in: RoundedRectangle(cornerRadius: 6))
        .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(model.selectedTab == id ? Color(nsColor: .separatorColor) : .clear))
        .contextMenu {
            if let id { Button("Close tab") { model.closeTab(id) } }
        }
    }
}

private struct SessionPages: NSViewControllerRepresentable {
    @ObservedObject var model: AppModel
    func makeNSViewController(context: Context) -> SessionPagesController { SessionPagesController() }
    func updateNSViewController(_ controller: SessionPagesController, context: Context) {
        var pages = [SessionPagesController.Page(id: "home", title: "Home") {
            NSHostingController(rootView: HomeContent().environmentObject(model))
        }]
        pages += model.viewerTabs.map { tab in
            SessionPagesController.Page(id: tab.id.uuidString, title: tab.name) { ViewerPageController(model: tab) }
        }
        controller.synchronize(pages, selected: model.selectedTab?.uuidString ?? "home")
        for tab in model.viewerTabs { tab.setVisible(tab.id == model.selectedTab) }
    }
}

private struct ViewerActions: View {
    @ObservedObject var model: ViewerModel
    var body: some View {
        Menu {
            Button("Fit to window") { model.sizing = .fit }
            Button("Actual size (1:1 pixels)") { model.sizing = .actualSize }
        } label: { Label("Display size", systemImage: "rectangle.expand.vertical") }
        .accessibilityLabel("Display size").help("Display size")
        Button { model.sendClipboard() } label: { Label("Send clipboard text", systemImage: "doc.on.clipboard") }
            .disabled(model.session?.phase != .active)
        Button { model.openFiles() } label: { Label("Files", systemImage: "folder") }
            .disabled(model.session?.phase != .active)
        Button { NSApp.keyWindow?.toggleFullScreen(nil) } label: { Label("Full screen", systemImage: "arrow.up.left.and.arrow.down.right") }
        Button { model.close() } label: { Label("Disconnect", systemImage: "xmark.circle") }
            .disabled(model.status == "Disconnected")
    }
}
