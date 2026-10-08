import Combine
import Foundation
import VnilinkCore

/// The file/job surface `FilesModel` needs. `VnilinkCore` conforms
/// through an owner-scoped adapter; tests use `FakeFilesBackend`.
protocol FilesBackend: Sendable {
    func fileRoots() throws -> [FfiFileRoot]
    func listDir(root: UInt64, path: String, offset: UInt64, limit: UInt16) throws -> FfiDirPage
    func createDir(root: UInt64, path: String, name: String) throws
    func renameEntry(root: UInt64, path: String, name: String, expected: FfiEntryToken?, newName: String) throws
    func fsList(sessionId: UInt64, generation: UInt64, root: UInt64, path: String, offset: UInt64, limit: UInt16) throws -> FfiDirPage
    func fsMkdir(sessionId: UInt64, generation: UInt64, root: UInt64, path: String, name: String) throws
    func fsRename(sessionId: UInt64, generation: UInt64, root: UInt64, path: String, name: String, expected: FfiEntryToken?, newName: String) throws
    func close()
    func queueOwner() -> String?
    func jobSubmitUpload(sessionId: UInt64, generation: UInt64, peerEndpointId: String, srcRoot: UInt64, srcRel: String, dstRoot: UInt64, dstRel: String) throws -> UInt64
    func jobSubmitDownload(sessionId: UInt64, generation: UInt64, peerEndpointId: String, srcRoot: UInt64, srcRel: String, dstRoot: UInt64, dstRel: String) throws -> UInt64
    func jobSnapshot(id: UInt64) throws -> FfiJobSnapshot
    func jobList() throws -> [FfiJobSnapshot]
    func jobPoll(id: UInt64) throws -> FfiJobSnapshot
    func jobCancel(id: UInt64) throws -> FfiJobSnapshot
    func jobResume(id: UInt64) throws -> FfiJobSnapshot
    func jobResolve(id: UInt64, choice: FfiConflictChoice, applyToRemaining: Bool) throws -> FfiJobSnapshot
    func jobForget(id: UInt64) throws
}

extension FilesBackend {
    func close() {}
    func queueOwner() -> String? { "test-backend" }
}

/// Which pane an action targets.
enum FilesSide: String, Identifiable {
    case local, remote
    var id: String { rawValue }
}

enum FilesSort: String, CaseIterable, Identifiable {
    case name = "Name", nameDescending = "Name, descending", size = "Size", modified = "Modified"
    var id: String { rawValue }
    func entries(_ entries: [FfiFsEntry]) -> [FfiFsEntry] {
        entries.sorted { a, b in
            if (a.kind == .dir) != (b.kind == .dir) { return a.kind == .dir }
            switch self {
            case .size: if a.size != b.size { return a.size > b.size }
            case .modified: if a.mtimeMs != b.mtimeMs { return a.mtimeMs > b.mtimeMs }
            default: break
            }
            let order = a.name.localizedStandardCompare(b.name)
            return self == .nameDescending ? order == .orderedDescending : order == .orderedAscending
        }
    }
}

struct FilesDrag: Codable {
    let window: UUID
    let remote: Bool
    let root: UInt64
    let path: String
    let names: [String]
}

/// One pane of the Files window: this computer or the remote computer.
struct FilesPane {
    var roots: [FfiFileRoot] = []
    var sort: FilesSort = .name
    var root: UInt64 = 0
    var path: [String] = []
    var page: FfiDirPage?
    var revision: UInt64 = 0
    var selection = Set<String>()
    var message: String?
    var pathString: String { path.joined(separator: "/") }
}

/// Two-pane file manager state. Interactive work (listings, mkdir, rename,
/// cancel, job snapshots) runs on `worker`; transfer polling runs on
/// `transferWorker` so a blocking `jobPoll` never queues cancellation or
/// browsing behind it. Only the newest outstanding listing per pane may paint.
@MainActor final class FilesModel: ObservableObject {
    @Published var local = FilesPane()
    @Published var remote = FilesPane()
    @Published var jobs: [FfiJobSnapshot] = []
    @Published var notice: String?
    @Published var failure: String?
    @Published var remoteRoot: UInt64 = 0
    @Published var remoteRoots: [UInt64] = [0]
    let localName: String
    let remoteName: String
    private let backend: any FilesBackend
    private let session: @MainActor () -> FfiSessionInfo?
    private let peerId: String
    private let defaults: UserDefaults
    private let dragWindow = UUID()
    private let worker = DispatchQueue(label: "com.vnilink.files", qos: .userInitiated)
    private let transferWorker = DispatchQueue(label: "com.vnilink.files.transfer", qos: .userInitiated)
    private var timer: Timer?
    private var lastTerminalJobs = Set<UInt64>()
    private var lastSessionStamp: String?
    private var stopped = false
    private var rootsLoading = false
    private var localRootsLoaded = false
    private var pendingUpload: URL?

    init(backend: any FilesBackend, session: @MainActor @escaping () -> FfiSessionInfo?, peerId: String, localName: String, remoteName: String, defaults: UserDefaults = .standard) {
        self.backend = backend
        self.session = session
        self.peerId = peerId
        self.localName = localName
        self.remoteName = remoteName
        self.defaults = defaults
        restoreLocation(remote: false)
        restoreLocation(remote: true)
    }

    private func locationKey(_ remote: Bool) -> String { "files.location.\(peerId).\(remote ? "remote" : "local")" }
    private func restoreLocation(remote isRemote: Bool) {
        guard let saved = defaults.dictionary(forKey: locationKey(isRemote)),
              let root = saved["root"] as? NSNumber, let path = saved["path"] as? [String],
              path.allSatisfy({ !$0.isEmpty && $0 != "." && $0 != ".." && !$0.contains("/") }) else { return }
        if isRemote { remoteRoot = root.uint64Value; remote.path = path }
        else { local.root = root.uint64Value; local.path = path }
    }
    private func rememberLocation(remote isRemote: Bool) {
        let pane = isRemote ? remote : local
        defaults.set(["root": NSNumber(value: isRemote ? remoteRoot : local.root), "path": pane.path], forKey: locationKey(isRemote))
    }

    func dragData(side: FilesSide, name: String) -> Data? {
        let pane = side == .remote ? remote : local
        let names = pane.selection.contains(name) ? pane.selection.sorted() : [name]
        let payload = FilesDrag(window: dragWindow, remote: side == .remote,
                                root: side == .remote ? remoteRoot : local.root, path: pane.pathString, names: names)
        return try? JSONEncoder().encode(payload)
    }

    /// A drop carries the original source location, so navigating during a
    /// drag cannot silently change which files are sent. Only this window's
    /// opposite pane accepts the payload; no paths from external apps run here.
    func dropData(_ data: Data, onto side: FilesSide, folder: String?) -> Bool {
        guard let payload = try? JSONDecoder().decode(FilesDrag.self, from: data),
              payload.window == dragWindow, payload.remote != (side == .remote),
              !payload.names.isEmpty,
              payload.names.allSatisfy({ !$0.isEmpty && $0 != "." && $0 != ".." && !$0.contains("/") }) else { return false }
        guard session()?.phase == .active else { return false }
        let target = side == .remote ? remote : local
        if let folder, !(target.page?.entries.contains(where: { $0.name == folder && $0.kind == .dir }) ?? false) { return false }
        let destination = target.pathString + (folder.map { target.pathString.isEmpty ? $0 : "/" + $0 } ?? "")
        transfer(download: payload.remote, names: payload.names, srcRoot: payload.root, srcBase: payload.path,
                 dstRoot: side == .remote ? remoteRoot : local.root, dstBase: destination)
        return true
    }

    /// Stage a desktop drop in the local pane. The user chooses the remote
    /// destination and confirms Send; there is no hidden automatic upload.
    func selectUpload(_ url: URL) {
        guard !stopped else { return }
        if !localRootsLoaded {
            pendingUpload = url
            refreshLocalRoots()
            return
        }
        let revision = bumpLocal()
        let backend = backend
        worker.async {
            let scoped = url.startAccessingSecurityScopedResource()
            defer { if scoped { url.stopAccessingSecurityScopedResource() } }
            let result = Result { () -> FilesSelection in
                let roots = try backend.fileRoots().map { ($0.id, URL(fileURLWithPath: $0.path)) }
                return try FilesSelection.locate(url, roots: roots)
            }
            DispatchQueue.main.async {
                guard !self.stopped, self.local.revision == revision else { return }
                switch result {
                case .success(let selection):
                    self.local.root = selection.root; self.local.path = selection.path
                    self.local.selection = [selection.name]
                    self.notice = "Choose the remote destination, then Send."
                    self.refreshLocal()
                case .failure(let error): self.failure = Self.message(error)
                }
            }
        }
    }

    func start() {
        guard !stopped else { return }
        refreshLocalRoots()
        discoverRemoteRoots()
        refreshRemote()
        refreshJobs()
        timer?.invalidate()
        timer = Timer.scheduledTimer(withTimeInterval: 1.0, repeats: true) { [weak self] _ in
            Task { @MainActor in self?.tick() }
        }
    }

    /// Discovers the remote host's approved folders by probing ids `0..<16`
    /// with a minimal listing. Host paths stay on the host; the picker shows
    /// `Shared folder {id}` so every configured root can be browsed and
    /// targeted instead of hardcoding zero.
    func discoverRemoteRoots() {
        guard let session = session(), session.phase == .active else { return }
        let backend = backend
        worker.async {
            var valid: [UInt64] = []
            for id in 0..<16 {
                do {
                    _ = try backend.fsList(sessionId: session.id, generation: session.generation, root: UInt64(id), path: "", offset: 0, limit: 1)
                    valid.append(UInt64(id))
                } catch {
                    // Unknown ids are skipped; denial/absence keeps found.
                    continue
                }
            }
            DispatchQueue.main.async {
                guard !self.stopped else { return }
                guard self.session()?.id == session.id, self.session()?.generation == session.generation else { return }
                if !valid.isEmpty {
                    self.remoteRoots = valid
                    if !valid.contains(self.remoteRoot) {
                        self.remoteRoot = valid[0]
                        self.remote.path.removeAll()
                    }
                    self.refreshRemote()
                }
            }
        }
    }

    func stop() {
        timer?.invalidate(); timer = nil
        stopped = true
        backend.close()
        _ = bumpRemote()
    }

    private func tick() {
        // A reconnect mints a new generation: late replies to the old one
        // must never paint, and the pane says so instead of showing stale
        // entries.
        let stamp = session().map { "\($0.id):\($0.generation):\(String(describing: $0.phase))" }
        if stamp != lastSessionStamp {
            lastSessionStamp = stamp
            _ = bumpRemote()
            if remote.page != nil {
                remote.page = nil
                remote.message = "The connection changed. Refresh the remote folder."
            }
            discoverRemoteRoots()
        }
        refreshJobs()
        // Transfers progress while the window is open: advance every
        // active job by one step.
        pollJobs()
    }

    // MARK: - Local pane

    func refreshLocalRoots() {
        guard !stopped, !rootsLoading else { return }
        rootsLoading = true
        let backend = backend
        worker.async {
            let result = Result { try backend.fileRoots() }
            DispatchQueue.main.async {
                guard !self.stopped else { return }
                switch result {
                case .success(let roots):
                    self.rootsLoading = false; self.localRootsLoaded = true
                    self.local.roots = roots
                    if roots.allSatisfy({ $0.id != self.local.root }) { self.local.root = roots.first?.id ?? 0; self.local.path.removeAll() }
                    if let upload = self.pendingUpload { self.pendingUpload = nil; self.selectUpload(upload) }
                    else { self.refreshLocal() }
                case .failure(let error): self.rootsLoading = false; self.failure = Self.message(error)
                }
            }
        }
    }

    func refreshLocal() {
        let revision = bumpLocal()
        let backend = backend
        let root = local.root, path = local.pathString
        worker.async {
            let result = Result { try Self.listAllLocal(backend: backend, root: root, path: path) }
            DispatchQueue.main.async {
                guard !self.stopped else { return }
                guard revision == self.local.revision else { return }
                switch result {
                case .success(let page):
                    self.local.page = page; self.local.message = nil
                    self.rememberLocation(remote: false)
                    self.local.selection = self.local.selection.intersection(page.entries.map(\.name))
                    if page.truncated {
                        self.local.message = "Showing first \(page.entries.count) entries."
                    }
                case .failure(let error):
                    if case FfiError.InvalidSetting = error, !self.local.path.isEmpty {
                        self.local.path.removeLast(); self.refreshLocal()
                    } else { self.local.message = Self.message(error) }
                }
            }
        }
    }

    /// Fetches every page so entries beyond the first 200 are visible.
    /// Pages stay bounded (500 rows each, up to the core's 5000-entry cap);
    /// `truncated` from the service is preserved for the notice above.
    nonisolated static func listAllLocal(backend: any FilesBackend, root: UInt64, path: String) throws -> FfiDirPage {
        var all: [FfiFsEntry] = []
        var offset: UInt64 = 0
        var total: UInt64 = 0
        var truncated = false
        var skipped: UInt64 = 0
        var first = true
        while true {
            let page = try backend.listDir(root: root, path: path, offset: offset, limit: 500)
            if first {
                total = page.total; truncated = page.truncated; skipped = page.nonUtf8Skipped
                first = false
            }
            all.append(contentsOf: page.entries)
            guard let next = page.nextOffset else { break }
            offset = next
            if all.count >= 5000 { break }
        }
        return FfiDirPage(entries: all, total: total, nextOffset: nil, truncated: truncated, nonUtf8Skipped: skipped)
    }

    func navigateLocal(to component: String) {
        local.path.append(component); local.selection.removeAll(); refreshLocal()
    }

    func goLocalParent() {
        guard !local.path.isEmpty else { return }
        local.path.removeLast(); local.selection.removeAll(); refreshLocal()
    }

    func mkdirLocal(name: String) {
        let backend = backend
        let root = local.root, path = local.pathString
        worker.async {
            let result = Result { try backend.createDir(root: root, path: path, name: name) }
            DispatchQueue.main.async {
                guard !self.stopped else { return }
                switch result {
                case .success: self.notice = "Folder created."; self.refreshLocal()
                case .failure(let error): self.failure = Self.message(error)
                }
            }
        }
    }

    func renameLocal(name: String, newName: String, token: FfiEntryToken?) {
        let backend = backend
        let root = local.root, path = local.pathString
        worker.async {
            let result = Result { try backend.renameEntry(root: root, path: path, name: name, expected: token, newName: newName) }
            DispatchQueue.main.async {
                guard !self.stopped else { return }
                switch result {
                case .success: self.notice = "Renamed."; self.refreshLocal()
                case .failure(let error): self.failure = Self.message(error)
                }
            }
        }
    }

    // MARK: - Remote pane

    func refreshRemote() {
        guard let session = session() else {
            remote.page = nil; remote.message = "Not connected."
            return
        }
        guard session.phase == .active else {
            remote.page = nil; remote.message = "The connection is not active."
            return
        }
        let revision = bumpRemote()
        let backend = backend
        let root = remoteRoot, path = remote.pathString
        worker.async {
            let result = Result { try Self.listAllRemote(backend: backend, session: session, root: root, path: path) }
            DispatchQueue.main.async {
                guard !self.stopped else { return }
                guard revision == self.remote.revision, self.session()?.id == session.id, self.session()?.generation == session.generation else { return }
                switch result {
                case .success(let page):
                    self.remote.page = page; self.remote.message = nil
                    self.rememberLocation(remote: true)
                    self.remote.selection = self.remote.selection.intersection(page.entries.map(\.name))
                    if page.truncated {
                        self.remote.message = "Showing first \(page.entries.count) entries."
                    }
                case .failure(let error):
                    if case FfiError.InvalidSetting = error, !self.remote.path.isEmpty {
                        self.remote.path.removeLast(); self.refreshRemote()
                    } else { self.remote.message = Self.message(error) }
                }
            }
        }
    }

    nonisolated static func listAllRemote(backend: any FilesBackend, session: FfiSessionInfo, root: UInt64, path: String) throws -> FfiDirPage {
        var all: [FfiFsEntry] = []
        var offset: UInt64 = 0
        var total: UInt64 = 0
        var truncated = false
        var skipped: UInt64 = 0
        var first = true
        while true {
            let page = try backend.fsList(sessionId: session.id, generation: session.generation, root: root, path: path, offset: offset, limit: 500)
            if first {
                total = page.total; truncated = page.truncated; skipped = page.nonUtf8Skipped
                first = false
            }
            all.append(contentsOf: page.entries)
            guard let next = page.nextOffset else { break }
            offset = next
            if all.count >= 5000 { break }
        }
        return FfiDirPage(entries: all, total: total, nextOffset: nil, truncated: truncated, nonUtf8Skipped: skipped)
    }

    func navigateRemote(to component: String) {
        remote.path.append(component); remote.selection.removeAll(); refreshRemote()
    }

    func goRemoteParent() {
        guard !remote.path.isEmpty else { return }
        remote.path.removeLast(); remote.selection.removeAll(); refreshRemote()
    }

    func mkdirRemote(name: String) {
        guard let session = session() else { return }
        let backend = backend
        let remoteName = remoteName
        let root = remoteRoot, path = remote.pathString
        worker.async {
            let result = Result { try backend.fsMkdir(sessionId: session.id, generation: session.generation, root: root, path: path, name: name) }
            DispatchQueue.main.async {
                guard !self.stopped else { return }
                switch result {
                case .success: self.notice = "Folder created on \(remoteName)."; self.refreshRemote()
                case .failure(let error): self.failure = Self.message(error)
                }
            }
        }
    }

    func renameRemote(name: String, newName: String, token: FfiEntryToken?) {
        guard let session = session() else { return }
        let backend = backend
        let remoteName = remoteName
        let root = remoteRoot, path = remote.pathString
        worker.async {
            let result = Result { try backend.fsRename(sessionId: session.id, generation: session.generation, root: root, path: path, name: name, expected: token, newName: newName) }
            DispatchQueue.main.async {
                guard !self.stopped else { return }
                switch result {
                case .success: self.notice = "Renamed on \(remoteName)."; self.refreshRemote()
                case .failure(let error): self.failure = Self.message(error)
                }
            }
        }
    }

    // MARK: - Transfers

    /// Sends each selected local item into the remote pane's folder.
    /// One job per item; the queue shows each one.
    func upload() {
        transfer(download: false, names: local.selection.sorted(), srcRoot: local.root, srcBase: local.pathString,
                 dstRoot: remoteRoot, dstBase: remote.pathString)
    }

    func download() {
        transfer(download: true, names: remote.selection.sorted(), srcRoot: remoteRoot, srcBase: remote.pathString,
                 dstRoot: local.root, dstBase: local.pathString)
    }

    private func transfer(download: Bool, names: [String], srcRoot: UInt64, srcBase: String, dstRoot: UInt64, dstBase: String) {
        guard !names.isEmpty, let session = session(), session.phase == .active else { return }
        let peer = peerId
        worker.async {
            let result = Result {
                for name in names {
                    let rel = srcBase.isEmpty ? name : "\(srcBase)/\(name)"
                    if download {
                        _ = try self.backend.jobSubmitDownload(sessionId: session.id, generation: session.generation, peerEndpointId: peer, srcRoot: srcRoot, srcRel: rel, dstRoot: dstRoot, dstRel: dstBase)
                    } else {
                        _ = try self.backend.jobSubmitUpload(sessionId: session.id, generation: session.generation, peerEndpointId: peer, srcRoot: srcRoot, srcRel: rel, dstRoot: dstRoot, dstRel: dstBase)
                    }
                }
            }
            DispatchQueue.main.async {
                guard !self.stopped else { return }
                switch result {
                case .success: self.notice = download ? "Download queued." : "Upload queued."
                case .failure(let error): self.failure = Self.message(error)
                }
                self.refreshJobs()
            }
        }
    }

    // MARK: - Queue

    func refreshJobs() {
        let backend = backend
        worker.async {
            let owner = backend.queueOwner()
            let result = Result { try backend.jobList() }
            DispatchQueue.main.async {
                guard !self.stopped, owner == backend.queueOwner() else { return }
                switch result {
                case .success(let jobs): self.adoptJobs(jobs)
                case .failure(let error): self.failure = Self.message(error)
                }
            }
        }
    }

    func pollJobs() {
        let active = jobs.filter { ![.complete, .cancelled, .failed].contains($0.state) && ($0.sender || $0.state != .waitingDecision) }.map(\.id)
        guard !active.isEmpty else { return }
        let backend = backend
        // Transfer polling runs off the interactive queue so a blocking
        // `jobPoll` never queues cancellation or browsing behind it. The
        // core's cancel watch lets `cancelJob` on `worker` interrupt this.
        transferWorker.async {
            let owner = backend.queueOwner()
            var snapshots: [FfiJobSnapshot] = []
            var failed: Error?
            for id in active {
                do { snapshots.append(try backend.jobPoll(id: id)) }
                catch { failed = error; break }
            }
            DispatchQueue.main.async {
                guard !self.stopped, owner == backend.queueOwner() else { return }
                if let failed { self.failure = Self.message(failed) }
                for snapshot in snapshots { self.adoptJob(snapshot) }
                self.refreshAfterTerminals()
            }
        }
    }

    func cancelJob(id: UInt64) {
        let backend = backend
        // Stays on the interactive queue: with polling on `transferWorker`
        // this reaches the backend even while a transfer blocks there.
        worker.async {
            let owner = backend.queueOwner()
            let result = Result { try backend.jobCancel(id: id) }
            DispatchQueue.main.async {
                guard !self.stopped, owner == backend.queueOwner() else { return }
                switch result {
                case .success(let snapshot): self.adoptJob(snapshot)
                case .failure(let error): self.failure = Self.message(error)
                }
            }
        }
    }

    func resumeJob(id: UInt64) {
        let backend = backend
        worker.async {
            let owner = backend.queueOwner()
            let result = Result { try backend.jobResume(id: id) }
            DispatchQueue.main.async {
                guard !self.stopped, owner == backend.queueOwner() else { return }
                switch result {
                case .success(let snapshot): self.adoptJob(snapshot)
                case .failure(let error): self.failure = Self.message(error)
                }
            }
        }
    }

    func forgetJob(id: UInt64) {
        let backend = backend
        worker.async {
            let owner = backend.queueOwner()
            let result = Result { try backend.jobForget(id: id) }
            DispatchQueue.main.async {
                guard !self.stopped, owner == backend.queueOwner() else { return }
                switch result {
                case .success: self.jobs.removeAll { $0.id == id }; self.refreshJobs()
                case .failure(let error): self.failure = Self.message(error)
                }
            }
        }
    }

    func resolveJob(id: UInt64, choice: FfiConflictChoice, applyToRemaining: Bool) {
        let backend = backend
        worker.async {
            let owner = backend.queueOwner()
            let result = Result { try backend.jobResolve(id: id, choice: choice, applyToRemaining: applyToRemaining) }
            DispatchQueue.main.async {
                guard !self.stopped, owner == backend.queueOwner() else { return }
                switch result {
                case .success(let snapshot): self.adoptJob(snapshot)
                case .failure(let error): self.failure = Self.message(error)
                }
            }
        }
    }

    private func adoptJobs(_ snapshots: [FfiJobSnapshot]) {
        jobs = snapshots.sorted { $0.id < $1.id }
        refreshAfterTerminals()
    }

    private func adoptJob(_ snapshot: FfiJobSnapshot) {
        if let index = jobs.firstIndex(where: { $0.id == snapshot.id }) { jobs[index] = snapshot }
        else { jobs.append(snapshot); jobs.sort { $0.id < $1.id } }
    }

    private func refreshAfterTerminals() {
        let terminal = Set(jobs.filter { [.complete, .cancelled, .failed].contains($0.state) }.map(\.id))
        let fresh = terminal.subtracting(lastTerminalJobs)
        lastTerminalJobs = terminal
        guard !fresh.isEmpty else { return }
        refreshLocal(); refreshRemote()
    }

    private func bumpLocal() -> UInt64 {
        local.revision &+= 1; return local.revision
    }

    private func bumpRemote() -> UInt64 {
        remote.revision &+= 1; return remote.revision
    }

    /// The listing token for a visible entry, for rename fencing.
    /// Returns nil when the entry is gone; the core then renames unfenced.
    func token(side: FilesSide, name: String) -> FfiEntryToken? {
        let page = side == .local ? local.page : remote.page
        return page?.entries.first { $0.name == name }?.token
    }

    static func message(_ error: Error) -> String {        switch error {
        case FfiError.TransferDenied: return "File access needs the transfer grant on that computer."
        case FfiError.FileExists: return "That name already exists here."
        case FfiError.StaleSession: return "The connection changed. Refresh and try again."
        case FfiError.NoSession: return "Not connected."
        case FfiError.Busy: return "The other computer is busy."
        case FfiError.Unsupported(let message), FfiError.InvalidSetting(let message): return message
        default: return error.localizedDescription
        }
    }

    static func stateLabel(_ state: FfiJobState) -> String {
        switch state {
        case .preparing: return "Preparing"
        case .serving: return "Serving"
        case .transferring: return "Transferring"
        case .waitingDecision: return "Needs a decision"
        case .interrupted: return "Interrupted"
        case .cancelled: return "Cancelled"
        case .failed: return "Failed"
        case .complete: return "Complete"
        }
    }
}
