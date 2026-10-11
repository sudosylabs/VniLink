import SwiftUI
import AppKit
import Carbon

final class AppDelegate: NSObject, NSApplicationDelegate {
    func applicationDidFinishLaunching(_ notification: Notification) {
        AppModel.shared.start()
        let loginLaunch = NSAppleEventManager.shared().currentAppleEvent?.paramDescriptor(forKeyword: AEKeyword(keyAELaunchedAsLogInItem)) != nil
        if loginLaunch && BackgroundSettings.shared.keepRunning {
            DispatchQueue.main.async { NSApp.windows.filter { $0.identifier?.rawValue == "computers" }.forEach { $0.orderOut(nil) } }
        }
    }
    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        if !flag { sender.windows.first { $0.identifier?.rawValue == "computers" }?.makeKeyAndOrderFront(nil) }
        return true
    }
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { !BackgroundSettings.shared.keepRunning }
    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        AppModel.shared.shutdown { sender.reply(toApplicationShouldTerminate: true) }
        return .terminateLater
    }
}

@main struct VniLinkApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) var delegate
    @StateObject private var model = AppModel.shared
    @StateObject private var background = BackgroundSettings.shared
    @Environment(\.openWindow) private var openWindow
    var body: some Scene {
        Window("VniLink", id: "computers") {
            MainWindow().environmentObject(model).frame(minWidth: 760, minHeight: 560)
                .task { model.start() }
                .onOpenURL { model.importFile($0) }
        }
        .windowToolbarStyle(.unifiedCompact)
        .defaultSize(width: 1080, height: 720)
        .commands {
            CommandGroup(replacing: .appSettings) {
                Button("Settings…") { model.showHome(); model.destination = .sharing; openWindow(id: "computers") }.keyboardShortcut(",")
            }
            #if DEBUG
            CommandMenu("Development") {
                Button("Preview devices") { model.loadPreviewDevices() }
            }
            #endif
            CommandGroup(after: .windowArrangement) {
                Button("Next Tab") { model.selectAdjacentTab(1) }.keyboardShortcut("]", modifiers: [.command, .shift])
                Button("Previous Tab") { model.selectAdjacentTab(-1) }.keyboardShortcut("[", modifiers: [.command, .shift])
            }
            CommandGroup(replacing: .saveItem) {
                Button(model.selectedTab == nil ? "Close Window" : "Close Tab") {
                    if NSApp.keyWindow?.identifier?.rawValue == "computers", let id = model.selectedTab {
                        model.closeTab(id)
                    } else { NSApp.keyWindow?.performClose(nil) }
                }.keyboardShortcut("w")
            }
            CommandGroup(after: .newItem) {
                Button("Add Computer…") { model.showHome(); model.adding = true }.keyboardShortcut("o")
                Button("Create Pairing File…") { model.createPairingFile() }.keyboardShortcut("n", modifiers: [.command, .shift])
            }
        }
        MenuBarExtra("VniLink", systemImage: "display") {
            Text(model.snapshot?.incoming?.phase == .active ? "A computer is connected" : (model.sharing ? "Screen sharing enabled" : "Screen sharing off"))
            Button("Open VniLink") { NSApp.activate(ignoringOtherApps: true); openWindow(id: "computers") }
            Button("Settings…") { model.showHome(); model.destination = .sharing; NSApp.activate(ignoringOtherApps: true); openWindow(id: "computers") }
            Divider()
            Button("Stop remote access") {
                model.setSharing(false)
                model.perform { try $0.stopHosting() }
            }.disabled(model.changingSharing || model.snapshot == nil)
            Divider()
            Button("Quit VniLink") { NSApp.terminate(nil) }.keyboardShortcut("q")
        }
    }
}
