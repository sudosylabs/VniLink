import SwiftUI
import VnilinkCore

struct DevicesPage: View {
    @EnvironmentObject var model: AppModel
    private var selected: FfiDeviceInfo? { model.snapshot?.devices.first { $0.id == model.selectedDeviceID } }
    var body: some View {
        Group {
            if model.snapshot?.devices.isEmpty != false {
                VStack(spacing: 14) {
                    Text("Add your first computer").font(.title2.weight(.semibold))
                    Text("Use a pairing file or invitation from your other computer.").foregroundStyle(.secondary)
                        .multilineTextAlignment(.center).frame(maxWidth: 320)
                    Button("Add computer…") { model.adding = true }.padding(.top, 8)
                }.padding(32).frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                List(selection: $model.selectedDeviceID) {
                    Section("Paired devices") {
                    ForEach(model.snapshot?.devices ?? []) { device in
                        Button { model.selectedDeviceID = device.id } label: {
                            DeviceRow(device: device)
                        }
                        .buttonStyle(.plain).tag(device.id)
                        .contextMenu {
                            Button("Connect") { model.connect(device) }
                            Button("Device settings") { model.selectedDeviceID = device.id }
                        }
                    }
                    }
                }.listStyle(.inset)
            }
        }
        .inspector(isPresented: Binding(get: { selected != nil }, set: { if !$0 { model.selectedDeviceID = nil } })) {
            if let device = selected {
                DeviceInspector(device: device).id(device.id)
                    .inspectorColumnWidth(min: 300, ideal: 360, max: 440)
                    .toolbar {
                        ToolbarItem(placement: .primaryAction) {
                            Button { model.selectedDeviceID = nil } label: { Label("Close device settings", systemImage: "sidebar.right") }
                                .help("Close device settings")
                        }
                    }
            }
        }
    }
}

private struct DeviceRow: View {
    let device: FfiDeviceInfo
    var body: some View {
        HStack(spacing: 12) {
            DeviceAvatar()
            VStack(alignment: .leading, spacing: 4) {
                Text(device.name).font(.body).lineLimit(1)
                Text(device.hostname ?? "Paired computer").font(.callout).foregroundStyle(.secondary).lineLimit(1)
            }
            Spacer(minLength: 12)
            Image(systemName: "chevron.right").font(.caption).foregroundStyle(.tertiary)
        }.padding(.vertical, 4).contentShape(Rectangle())
    }
}

struct DeviceInspector: View {
    @EnvironmentObject var model: AppModel
    let device: FfiDeviceInfo
    @State private var name = ""
    @State private var control = false
    @State private var clipboard = false
    @State private var files = false
    @State private var forgetting = false
    var body: some View {
        Form {
            Section {
                VStack(alignment: .leading, spacing: 10) {
                    DeviceAvatar(size: 52)
                    Text(device.name).font(.title3.weight(.semibold))
                    if let hostname = device.hostname {
                        Text(hostname).font(.callout).foregroundStyle(.secondary).textSelection(.enabled)
                    }
                }.padding(.vertical, 4)
                HStack {
                    Button("Connect") { model.connect(device) }.buttonStyle(.borderedProminent)
                    Button("Files…") { model.openFiles(device) }
                        .help("Browse both computers and copy files without opening the desktop viewer or requiring screen sharing.")
                }
            }
            Section("Name on this Mac") {
                TextField("Local name", text: $name)
                Text("Leave empty to use the computer's hostname.").font(.callout).foregroundStyle(.secondary)
                Button("Save name") {
                    let value = name.trimmingCharacters(in: .whitespacesAndNewlines)
                    model.perform({ try $0.setPeerPetName(endpointId: device.id, name: value.isEmpty ? nil : value) }, success: "Device name saved.")
                }.disabled(name == (device.petName ?? ""))
            }
            Section("Access to this Mac") {
                Toggle("Desktop control", isOn: $control)
                Toggle("Clipboard text", isOn: $clipboard)
                Toggle("File transfer", isOn: $files)
                Button("Save permissions") {
                    model.perform({ try $0.authorize(endpointId: device.id, control: control, clipboard: clipboard, transfer: files) }, success: "Device permissions saved.")
                }.disabled(control == device.grants.control && clipboard == device.grants.clipboard && files == device.grants.transfer)
                Button("Revoke access") { model.perform { try $0.revoke(endpointId: device.id) } }
                    .disabled(!device.grants.control && !device.grants.clipboard && !device.grants.transfer)
            }
            Section {
                DisclosureGroup("Identity") {
                    Text(device.endpointId).font(.system(.caption, design: .monospaced)).textSelection(.enabled)
                }
                Button("Forget device…", role: .destructive) { forgetting = true }
            }
        }.formStyle(.grouped)
        .onAppear { load(device) }
        .onChange(of: device) { old, new in
            if name == (old.petName ?? "") { name = new.petName ?? "" }
            if control == old.grants.control && clipboard == old.grants.clipboard && files == old.grants.transfer {
                control = new.grants.control; clipboard = new.grants.clipboard; files = new.grants.transfer
            }
        }
        .confirmationDialog("Forget \(device.name)?", isPresented: $forgetting, titleVisibility: .visible) {
            Button("Forget device", role: .destructive) {
                model.perform({ try $0.unpair(endpointId: device.id) }, completed: { model.selectedDeviceID = nil })
            }
        } message: { Text("It will lose access to this Mac. Pair it again to reconnect.") }
    }
    private func load(_ device: FfiDeviceInfo) {
        name = device.petName ?? ""; control = device.grants.control
        clipboard = device.grants.clipboard; files = device.grants.transfer
    }
}
