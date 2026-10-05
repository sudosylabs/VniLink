import SwiftUI
import AppKit
import ApplicationServices
import ServiceManagement
import VnilinkCore

enum AppSection: String, CaseIterable, Identifiable {
    case devices = "Devices", sharing = "This Mac", connection = "Connection", keyboard = "Keyboard", files = "Files", pairing = "Pairing"
    var id: String { rawValue }
    var icon: String {
        switch self {
        case .devices: return "display.2"
        case .sharing: return "desktopcomputer"
        case .connection: return "network"
        case .keyboard: return "keyboard"
        case .files: return "folder"
        case .pairing: return "link"
        }
    }
}

struct SettingsContent: View {
    let section: AppSection
    var body: some View {
        Group {
            switch section {
            case .sharing: SharingSettings()
            case .connection: ConnectionSettings()
            case .keyboard: KeyboardSettings()
            case .files: FileSettings()
            case .pairing: PairingSettings()
            case .devices: EmptyView()
            }
        }
    }
}
private struct RelayDraft: Identifiable {
    let id = UUID()
    var url = ""
    var authenticated = false
    var token = ""
}
private struct ConnectionSettings: View {
    @EnvironmentObject var model: AppModel
    @State private var publicRelays = true
    @State private var relays: [RelayDraft] = []
    @State private var loaded = false
    var body: some View {
        Form {
            Section("Relays") {
                Toggle("Use public relays", isOn: $publicRelays)
                Text("Changes apply after restarting VniLink.").foregroundStyle(.secondary)
            }
            if !publicRelays {
                ForEach($relays) { $relay in
                    Section("Custom relay") {
                        TextField("HTTPS URL", text: $relay.url)
                        Toggle("Authentication", isOn: $relay.authenticated)
                        if relay.authenticated {
                            SecureField("New authentication token", text: $relay.token)
                            Text("Leave the token empty to keep the saved credential for this URL.").font(.callout).foregroundStyle(.secondary)
                        }
                        Button("Remove relay", role: .destructive) { relays.removeAll { $0.id == relay.id } }
                    }
                }
                Button("Add relay") { relays.append(RelayDraft()) }.disabled(relays.count >= 8)
            }
            Button("Save relay settings") {
                if publicRelays { model.perform({ try $0.setRelayPublic() }, success: "Saved. Restart VniLink to use these relays.") }
                else {
                    let inputs = relays.map { FfiRelayInput(url: $0.url, authenticated: $0.authenticated, replacementToken: $0.token) }
                    model.perform({ try $0.saveCustomRelays(inputs: inputs) }, success: "Saved. Restart VniLink to use these relays.", completed: { for index in relays.indices { relays[index].token = "" } })
                }
            }
        }.formStyle(.grouped)
        .onReceive(model.$savedRelays) { saved in
            guard !loaded, model.snapshot != nil else { return }
            publicRelays = saved.isEmpty
            relays = saved.map { RelayDraft(url: $0.url, authenticated: $0.authenticated) }
            if relays.isEmpty { relays = [RelayDraft()] }
            loaded = true
        }
    }
}

private struct SharingSettings: View {
    @EnvironmentObject var model: AppModel
    @StateObject private var background = BackgroundSettings.shared
    @State private var reset = false
    @State private var screenPermission = CGPreflightScreenCaptureAccess()
    @State private var inputPermission = AXIsProcessTrusted()
    var body: some View {
        Form {
            Section("Remote access") {
                Toggle("Allow connections", isOn: Binding(get: { model.snapshot?.hosting ?? false }, set: { enabled in
                    model.perform { if enabled { try $0.startHosting() } else { try $0.stopHosting() } }
                }))
            }
            Section("Background and startup") {
                Toggle("Keep running when the window closes", isOn: $background.keepRunning)
                Toggle("Launch at login", isOn: Binding(get: { background.launchesAtLogin }, set: { background.setLogin($0) })).disabled(model.isPreview)
                Toggle("Resume sharing and allow connections on launch", isOn: $background.resumeSharing).disabled(model.isPreview)
                Toggle("Resume remote input on launch", isOn: $background.resumeInput).disabled(model.isPreview)
                Text("The menu bar keeps Open, Stop remote access and Quit available when the window is closed. Sharing and input resume only when their existing macOS permissions allow it; paired-device permissions still apply.").foregroundStyle(.secondary)
                if background.loginStatus == .requiresApproval {
                    Text("Allow VniLink in Login Items to finish enabling launch at login.").foregroundStyle(.secondary)
                    Button("Open Login Items…") { SMAppService.openSystemSettingsLoginItems() }
                }
            }
            Section("macOS permissions") {
                LabeledContent("Screen Recording", value: screenPermission ? "Allowed" : "Not allowed")
                LabeledContent("Accessibility", value: inputPermission ? "Allowed" : "Not allowed")
                Toggle("Share the main display", isOn: Binding(get: { model.sharing }, set: { model.setSharing($0) })).disabled(model.changingSharing)
                Text("Sharing captures the main display while this app is open. Paired computers still need your permission to connect.").foregroundStyle(.secondary)
                if model.changingSharing { ProgressView("Updating screen sharing…") }
                Button(model.inputEnabled ? "Remote input enabled" : "Enable remote keyboard and mouse") { model.enableInput() }.disabled(model.inputEnabled)
                Text("Enable Accessibility to let an authorized computer control this Mac. Use the startup options above to resume sharing and input on future launches.").foregroundStyle(.secondary)
                Button("Open System Settings…") { NSWorkspace.shared.open(URL(fileURLWithPath: "/System/Applications/System Settings.app")) }
            }
            Section("Identity") {
                Text(model.snapshot?.endpointId ?? "Opening…").font(.system(.caption, design: .monospaced)).textSelection(.enabled)
                Button("Reset identity…", role: .destructive) { reset = true }.disabled(model.isPreview)
            }
        }.formStyle(.grouped)
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
            background.refresh()
            screenPermission = CGPreflightScreenCaptureAccess(); inputPermission = AXIsProcessTrusted()
        }
        .confirmationDialog("Reset this Mac's identity?", isPresented: $reset) {
            Button("Reset identity", role: .destructive) {
                model.resetIdentity()
            }
        } message: { Text("Your paired computers will be forgotten. Relay settings are kept. This Mac will receive a new identity.") }
    }
}
private struct KeyboardSettings: View {
    @EnvironmentObject var model: AppModel
    var body: some View {
        Form {
            Section("Keyboard") {
                Picker("Mode", selection: Binding(get: { model.keyboardMode }, set: { mode in model.perform { try $0.setKeyboardMode(mode: mode) } })) {
                    Text("Characters").tag(FfiKeyboardMode.character)
                    Text("Physical keys").tag(FfiKeyboardMode.map)
                    Text("Translate on host").tag(FfiKeyboardMode.translate)
                }
                Text("Character mode keeps what you type. Physical mode keeps key positions. Translation currently requires a Linux X11 host.").foregroundStyle(.secondary)
                Toggle("Swap Control and Command", isOn: Binding(get: { model.swapModifiers }, set: { enabled in model.perform { try $0.setSwapControlAndCommand(enabled: enabled) } }))
                Text("Keyboard preferences apply to this launch.").foregroundStyle(.secondary)
            }
        }.formStyle(.grouped)
    }
}
private struct FileSettings: View {
    @EnvironmentObject var model: AppModel
    @State private var folder: URL?
    var body: some View {
        Form {
            Section("Received files") {
                LabeledContent("Folder", value: folder?.path ?? "Not selected")
                Text("Choose a folder for this launch. Each computer needs its own file-transfer permission. Transfers are limited to 8 MiB.").foregroundStyle(.secondary)
                Button("Choose folder…") {
                    let panel = NSOpenPanel(); panel.canChooseFiles = false; panel.canChooseDirectories = true; panel.allowsMultipleSelection = false
                    if panel.runModal() == .OK, let url = panel.url {
                        model.perform({ try $0.setTransferDirectory(path: url.path) }, success: "Receive folder selected for this launch.")
                        folder = url
                    }
                }
            }
        }.formStyle(.grouped)
    }
}
private struct PairingSettings: View {
    @EnvironmentObject var model: AppModel
    @State private var token = ""
    var body: some View {
        Form {
            Section("Pairing files") {
                Text("Create a file for your other computer, or open one it created.").foregroundStyle(.secondary)
                Button("Create pairing file…") { model.createPairingFile() }
                Button("Open pairing file…") { model.choosePairingFile() }
            }
            Section("Invitation tokens") {
                Text("Use a one-time invitation instead of a pairing file. Share it only with the computer you want to pair.").foregroundStyle(.secondary)
                SecureField("Invitation token", text: $token)
                Button("Accept invitation") { model.acceptToken(token, completed: { token = "" }) }.disabled(token.isEmpty)
                Button("Copy a new invitation") { model.copyInvitation() }
            }
        }.formStyle(.grouped)
    }
}
