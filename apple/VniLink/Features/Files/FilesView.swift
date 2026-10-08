import SwiftUI
import UniformTypeIdentifiers
import VnilinkCore

extension FfiJobSnapshot: Identifiable {}

private let filesDragType = UTType(exportedAs: "com.vnilink.files-selection", conformingTo: .data)

struct FilesView: View {
    @ObservedObject var model: FilesModel
    @State private var mkdirSide: FilesSide?
    @State private var mkdirName = ""
    @State private var renameTarget: RenameTarget?
    @State private var renameName = ""
    @State private var conflictJob: FfiJobSnapshot?
    @State private var conflictChoice: FfiConflictChoice = .skip
    @State private var conflictApplyRemaining = false

    var body: some View {
        VStack(spacing: 0) {
            if let failure = model.failure {
                HStack {
                    Text(failure).font(.callout).foregroundStyle(.red)
                    Spacer()
                    Button("Dismiss") { model.failure = nil }
                }.padding(.horizontal, 16).padding(.vertical, 8)
                Divider()
            }
            HStack(spacing: 12) {
                Picker("This computer", selection: Binding(
                    get: { model.local.root },
                    set: { model.local.root = $0; model.local.path.removeAll(); model.refreshLocal() }
                )) {
                    ForEach(model.local.roots, id: \.id) { root in
                        Text(root.path).tag(root.id)
                    }
                }.frame(maxWidth: .infinity)
                Picker("Remote computer", selection: Binding(
                    get: { model.remoteRoot },
                    set: { model.remoteRoot = $0; model.remote.path.removeAll(); model.refreshRemote() }
                )) {
                    ForEach(model.remoteRoots, id: \.self) { id in
                        Text("Shared folder \(id)").tag(id)
                    }
                }.frame(maxWidth: .infinity)
            }.padding(.horizontal, 12).padding(.vertical, 6)
            Divider()
            HSplitView {
                FilesPaneView(
                    title: model.localName,
                    subtitle: "This computer",
                    pane: model.local,
                    canGoParent: !model.local.path.isEmpty,
                    onParent: { model.goLocalParent() },
                    onRefresh: { model.refreshLocal() },
                    onEnter: { model.navigateLocal(to: $0) },
                    onMkdir: { mkdirSide = .local; mkdirName = "" },
                    onRename: { renameTarget = RenameTarget(side: .local, name: $0); renameName = $0 },
                    onSend: nil,
                    sendTitle: "",
                    selection: $model.local.selection,
                    sort: $model.local.sort,
                    dragData: { model.dragData(side: .local, name: $0) },
                    onDrop: { model.dropData($0, onto: .local, folder: $1) }
                ).frame(minWidth: 280)
                FilesPaneView(
                    title: model.remoteName,
                    subtitle: "Remote computer",
                    pane: model.remote,
                    canGoParent: !model.remote.path.isEmpty,
                    onParent: { model.goRemoteParent() },
                    onRefresh: { model.refreshRemote() },
                    onEnter: { model.navigateRemote(to: $0) },
                    onMkdir: { mkdirSide = .remote; mkdirName = "" },
                    onRename: { renameTarget = RenameTarget(side: .remote, name: $0); renameName = $0 },
                    onSend: nil,
                    sendTitle: "",
                    selection: $model.remote.selection,
                    sort: $model.remote.sort,
                    dragData: { model.dragData(side: .remote, name: $0) },
                    onDrop: { model.dropData($0, onto: .remote, folder: $1) }
                ).frame(minWidth: 280)
            }
            Divider()
            FilesTransferBar(model: model)
            Divider()
            FilesQueueView(model: model, conflictJob: $conflictJob)
        }
        .sheet(item: $mkdirSide) { side in
            VStack(spacing: 12) {
                Text("New folder").font(.headline)
                TextField("Folder name", text: $mkdirName)
                    .textFieldStyle(.roundedBorder).frame(width: 260)
                HStack {
                    Button("Cancel") { mkdirSide = nil }
                    Button("Create") {
                        if side == .local { model.mkdirLocal(name: mkdirName) }
                        else { model.mkdirRemote(name: mkdirName) }
                        mkdirSide = nil
                    }.keyboardShortcut(.defaultAction).disabled(mkdirName.isEmpty)
                }
            }.padding(20)
        }
        .sheet(item: $renameTarget) { target in
            VStack(spacing: 12) {
                Text("Rename “\(target.name)”").font(.headline)
                TextField("New name", text: $renameName)
                    .textFieldStyle(.roundedBorder).frame(width: 260)
                HStack {
                    Button("Cancel") { renameTarget = nil }
                    Button("Rename") {
                        if target.side == .local {
                            model.renameLocal(name: target.name, newName: renameName, token: model.token(side: target.side, name: target.name))
                        } else {
                            model.renameRemote(name: target.name, newName: renameName, token: model.token(side: target.side, name: target.name))
                        }
                        renameTarget = nil
                    }.keyboardShortcut(.defaultAction).disabled(renameName.isEmpty || renameName == target.name)
                }
            }.padding(20)
        }
        .sheet(item: $conflictJob) { job in
            FilesConflictSheet(model: model, job: job, choice: $conflictChoice, applyRemaining: $conflictApplyRemaining) {
                conflictJob = nil
            }
        }
        .onAppear { model.start() }
        .onDisappear { model.stop() }
    }
}

struct RenameTarget: Identifiable {
    let side: FilesSide
    let name: String
    var id: String { "\(side.rawValue)/\(name)" }
}

private struct FilesPaneView: View {
    let title: String
    let subtitle: String
    let pane: FilesPane
    let canGoParent: Bool
    let onParent: () -> Void
    let onRefresh: () -> Void
    let onEnter: (String) -> Void
    let onMkdir: () -> Void
    let onRename: (String) -> Void
    let onSend: (() -> Void)?
    let sendTitle: String
    @Binding var selection: Set<String>
    @Binding var sort: FilesSort
    let dragData: (String) -> Data?
    let onDrop: (Data, String?) -> Bool

    private func receive(_ providers: [NSItemProvider], folder: String?) -> Bool {
        guard let provider = providers.first(where: { $0.hasItemConformingToTypeIdentifier(filesDragType.identifier) }) else { return false }
        provider.loadDataRepresentation(forTypeIdentifier: filesDragType.identifier) { data, _ in
            guard let data else { return }
            DispatchQueue.main.async { _ = onDrop(data, folder) }
        }
        return true
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                VStack(alignment: .leading, spacing: 2) {
                    Text(title).font(.headline).lineLimit(1)
                    Text(subtitle).font(.caption).foregroundStyle(.secondary)
                }
                Spacer()
                Button { onParent() } label: { Label("Parent folder", systemImage: "chevron.up") }
                    .disabled(!canGoParent).help("Go to the parent folder")
                Button { onRefresh() } label: { Label("Refresh", systemImage: "arrow.clockwise") }
                    .help("Refresh this folder")
                Button { onMkdir() } label: { Label("New folder", systemImage: "folder.badge.plus") }
                    .help("Create a folder here")
            }.padding(.horizontal, 12).padding(.vertical, 8)
            HStack {
                Text(pane.path.isEmpty ? "/" : "/" + pane.path.joined(separator: "/"))
                    .font(.caption).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle)
                Spacer()
                Picker("Sort", selection: $sort) {
                    ForEach(FilesSort.allCases) { order in Text(order.rawValue).tag(order) }
                }.labelsHidden().frame(width: 145)
            }.padding(.horizontal, 12).padding(.bottom, 4)
            Divider()
            if let message = pane.message {
                VStack(spacing: 8) {
                    Spacer()
                    Text(message).foregroundStyle(.secondary).multilineTextAlignment(.center)
                    Button("Retry") { onRefresh() }
                    Spacer()
                }.padding()
            } else if let page = pane.page, page.entries.isEmpty {
                VStack(spacing: 8) {
                    Spacer()
                    Text("This folder is empty.").foregroundStyle(.secondary)
                    Spacer()
                }.padding()
            } else {
                List(selection: $selection) {
                    ForEach(sort.entries(pane.page?.entries ?? []), id: \.name) { entry in
                        FileEntryRow(entry: entry)
                            .tag(entry.name)
                            .onDrag {
                                let provider = NSItemProvider()
                                if let data = dragData(entry.name) {
                                    provider.registerDataRepresentation(forTypeIdentifier: filesDragType.identifier, visibility: .ownProcess) { completion in
                                        completion(data, nil); return nil
                                    }
                                }
                                return provider
                            }
                            .onDrop(of: [filesDragType], isTargeted: nil) { providers in
                                entry.kind == .dir && receive(providers, folder: entry.name)
                            }
                            .contextMenu {
                                if entry.kind == .dir {
                                    Button("Open") { onEnter(entry.name) }
                                }
                                Button("Rename…") { onRename(entry.name) }
                            }
                            .onTapGesture(count: 2) {
                                if entry.kind == .dir { onEnter(entry.name) }
                            }
                    }
                    .listStyle(.inset)
                }
            }
        }
        .onDrop(of: [filesDragType], isTargeted: nil) { receive($0, folder: nil) }
    }
}

private struct FileEntryRow: View {
    let entry: FfiFsEntry
    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: entry.kind == .dir ? "folder" : "doc")
                .foregroundStyle(entry.kind == .dir ? Color.accentColor : Color.secondary)
            VStack(alignment: .leading, spacing: 1) {
                Text(entry.name).lineLimit(1)
                if entry.kind == .file {
                    Text(ByteCountFormatter.string(fromByteCount: Int64(entry.size), countStyle: .file))
                        .font(.caption).foregroundStyle(.secondary)
                }
            }
            Spacer()
        }
    }
}

private struct FilesTransferBar: View {
    @ObservedObject var model: FilesModel
    var body: some View {
        HStack(spacing: 12) {
            Button {
                model.upload()
            } label: {
                Label("Send to \(model.remoteName)", systemImage: "arrow.right")
            }
            .disabled(model.local.selection.isEmpty)
            .help("Copy the selected items to the remote folder")
            Button {
                model.download()
            } label: {
                Label("Receive from \(model.remoteName)", systemImage: "arrow.left")
            }
            .disabled(model.remote.selection.isEmpty)
            .help("Copy the selected items to this computer")
            if let notice = model.notice {
                Text(notice).font(.caption).foregroundStyle(.secondary).lineLimit(1)
            }
            Spacer()
            Button("Refresh all") {
                model.refreshLocal(); model.refreshRemote(); model.refreshJobs()
            }
        }.padding(.horizontal, 16).padding(.vertical, 8)
    }
}

private struct FilesQueueView: View {
    @ObservedObject var model: FilesModel
    @Binding var conflictJob: FfiJobSnapshot?
    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Text("Transfer queue").font(.headline)
                Spacer()

            }.padding(.horizontal, 16).padding(.vertical, 6)
            if model.jobs.isEmpty {
                Text("No transfers yet. Select files above and send or receive them.")
                    .font(.callout).foregroundStyle(.secondary).padding(.bottom, 8)
            } else {
                List(model.jobs, id: \.id) { job in
                    FilesJobRow(model: model, job: job, conflictJob: $conflictJob)
                }.frame(minHeight: 90, maxHeight: 220)
            }
        }
    }
}

private struct FilesJobRow: View {
    @ObservedObject var model: FilesModel
    let job: FfiJobSnapshot
    @Binding var conflictJob: FfiJobSnapshot?
    var body: some View {
        HStack(spacing: 10) {
            VStack(alignment: .leading, spacing: 2) {
                Text(job.label.isEmpty ? "Transfer #\(job.id)" : job.label).lineLimit(1)
                Text("\(job.direction == .upload ? "Upload" : "Download") • \(FilesModel.stateLabel(job.state)) • \(job.filesDone)/\(job.filesTotal) files")
                    .font(.caption).foregroundStyle(.secondary)
                if job.totalBytes > 0 {
                    ProgressView(value: Double(job.doneBytes), total: Double(job.totalBytes))
                        .progressViewStyle(.linear).frame(maxWidth: 260)
                }
                if let error = job.error {
                    Text(error).font(.caption).foregroundStyle(.red).lineLimit(2)
                }
                if job.resumedFiles > 0 {
                    Text("Resumed \(job.resumedFiles) file(s) without re-downloading.").font(.caption).foregroundStyle(.secondary)
                }
            }
            Spacer()
            switch job.state {
            case .waitingDecision:
                Button("Decide…") { conflictJob = job }
            case .transferring, .serving, .preparing:
                Button("Cancel") { model.cancelJob(id: job.id) }
            case .interrupted:
                Button("Resume") { model.resumeJob(id: job.id) }
            case .cancelled, .failed:
                HStack {
                    Button("Resume") { model.resumeJob(id: job.id) }
                    Button("Forget") { model.forgetJob(id: job.id) }
                }
            case .complete:
                Button("Forget") { model.forgetJob(id: job.id) }
            }
        }.padding(.vertical, 2)
    }
}

private struct FilesConflictSheet: View {
    @ObservedObject var model: FilesModel
    let job: FfiJobSnapshot
    @Binding var choice: FfiConflictChoice
    @Binding var applyRemaining: Bool
    let done: () -> Void
    var body: some View {
        VStack(spacing: 12) {
            Text("Name already exists").font(.headline)
            Text("“\(job.waitingFile ?? "A file")” already exists at the destination. The original stays untouched unless you replace it.")
                .multilineTextAlignment(.center).frame(width: 300)
            Picker("Choice", selection: $choice) {
                Text("Skip").tag(FfiConflictChoice.skip)
                Text("Replace").tag(FfiConflictChoice.replace)
                Text("Keep both").tag(FfiConflictChoice.keepBoth)
            }.pickerStyle(.radioGroup).frame(width: 200)
            Toggle("Apply to the remaining conflicts", isOn: $applyRemaining)
            HStack {
                Button("Cancel") { done() }
                Button("Apply") {
                    model.resolveJob(id: job.id, choice: choice, applyToRemaining: applyRemaining)
                    done()
                }.keyboardShortcut(.defaultAction)
            }
        }.padding(20)
    }
}
