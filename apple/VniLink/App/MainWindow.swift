import SwiftUI
import VnilinkCore

extension FfiPairingPreview: Identifiable {}

struct MainWindow: View {
    @EnvironmentObject var model: AppModel
    var body: some View {
        NavigationSplitView {
            List(selection: Binding<AppSection?>(get: { model.destination }, set: { if let section = $0 { Task { @MainActor in model.destination = section } } })) {
                Label(AppSection.devices.rawValue, systemImage: AppSection.devices.icon).tag(AppSection.devices)
                Section("Settings") {
                    ForEach(AppSection.allCases.filter { $0 != .devices }) { section in
                        Label(section.rawValue, systemImage: section.icon).tag(section)
                    }
                }
            }
            .listStyle(.sidebar)
            .navigationSplitViewColumnWidth(min: 180, ideal: 210, max: 260)
        } detail: {
            NavigationStack {
                VStack(spacing: 0) {
                    if model.snapshot != nil {
                        if model.destination == .devices { DevicesPage() }
                        else { SettingsContent(section: model.destination) }
                    } else {
                        VStack(spacing: 18) {
                            if model.error == nil { ProgressView("Opening your computers…") }
                            else {
                                Text("VniLink could not open").font(.title2.bold())
                                Text(model.error ?? "").foregroundStyle(.secondary)
                                Button("Try again") { model.error = nil; model.start() }
                            }
                        }.padding(36).frame(maxWidth: .infinity, maxHeight: .infinity)
                    }
                    if model.notice != nil || model.isPreview {
                        Divider()
                        HStack(spacing: 16) {
                            if let notice = model.notice { Text(notice) }
                            Spacer(minLength: 0)
                            if model.isPreview { Text("Preview data · no Keychain changes") }
                        }.font(.callout).foregroundStyle(.secondary).padding(.horizontal, 20).padding(.vertical, 12)
                    }
                }
                .navigationTitle(model.destination.rawValue)
                .toolbar {
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
        .sheet(isPresented: $model.adding) { AddComputerSheet().environmentObject(model) }
        .sheet(item: $model.preview) { _ in
            CodeSheet(title: "Add computer", detail: "Enter the three-digit code shown on the computer that created this file.", action: "Add computer") { model.confirmPairing($0) }
        }
        .sheet(isPresented: Binding(get: { model.offerCode != nil }, set: { if !$0 { model.offerCode = nil } })) {
            VStack(alignment: .leading, spacing: 24) {
                Text("Pair this computer").font(.title2.bold())
                Text("Open the saved pairing file on your other computer, then enter this code there.").foregroundStyle(.secondary)
                Text(model.offerCode ?? "").font(.system(size: 44, weight: .medium, design: .monospaced)).tracking(10).textSelection(.enabled)
                Text("The file expires after 15 minutes. Pairing alone does not grant desktop access.").font(.callout).foregroundStyle(.secondary)
                HStack { Spacer(); Button("Done") { model.offerCode = nil }.keyboardShortcut(.defaultAction) }
            }.padding(28).frame(width: 420)
        }
        .sheet(isPresented: Binding(get: { model.request != nil }, set: { if !$0, model.request != nil { model.perform { try $0.rejectControlRequest() } } })) {
            CodeSheet(title: "Access request", detail: "\(model.snapshot?.devices.first(where: { $0.id == model.request?.endpointId })?.name ?? "A paired computer") is asking to control your desktop. Enter the code shown on its screen.", action: "Allow control") { digits in
                model.perform { _ = try $0.confirmControlRequest(digits: digits) }
            }
        }
        .alert("Could not complete the action", isPresented: Binding(get: { model.error != nil && model.snapshot != nil }, set: { if !$0 { model.error = nil } })) {
            Button("OK") { model.error = nil }
        } message: { Text(model.error ?? "") }
    }
}
