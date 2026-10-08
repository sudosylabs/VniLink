import AppKit
import Combine
import ApplicationServices
import UniformTypeIdentifiers
import VnilinkCore

extension FfiDeviceInfo: Identifiable {
    public var id: String { endpointId }
    var name: String { petName ?? hostname ?? "Computer \(endpointId.prefix(6).uppercased())" }
}
extension UTType { static let pairing = UTType(exportedAs: "com.vnilink.pairing") }

@MainActor final class AppModel: ObservableObject {
    static let shared = AppModel()
    @Published var snapshot: FfiSnapshot?
    @Published var error: String?
    @Published var notice: String?
    @Published private(set) var isPreview = (ProcessInfo.processInfo.arguments.contains("--preview") || Bundle.main.bundleIdentifier == "com.vnilink.tabs-preview")
    @Published var destination: AppSection = .devices
    @Published var selectedDeviceID: String?
    @Published var adding = false
    @Published var preview: FfiPairingPreview?
    @Published var offerCode: String?
    @Published var request: FfiControlRequest?
    @Published var savedRelays: [FfiRelay] = []
    @Published var keyboardMode: FfiKeyboardMode = .character
    @Published var swapModifiers = false
    @Published var sharing = false
    @Published var changingSharing = false
    @Published var inputEnabled = false
    @Published private(set) var receiveFolder = UserDefaults.standard.url(forKey: "receiveFolder") ?? DesktopFiles.defaultFolder
    @Published private(set) var fileRoots = AppModel.defaultFileRoots
    static var blobDirectory: URL {
        FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("VniLink/blobstore", isDirectory: true)
    }
    /// Folders the Files window may browse on this Mac. Starts empty so
    /// nothing is browsable until the owner deliberately chooses folders
    /// in Settings; a receive-directory grant never widens into
    /// whole-filesystem access. Shown verbatim there, never logged.
    static var fileRoots: [String] { defaultFileRoots }
    private static var defaultFileRoots: [String] {
        let saved = UserDefaults.standard.stringArray(forKey: "fileRoots")
        guard let saved else { return [] }
        return saved
    }
    private var events: EventPoller?
    private var lastRemoteClipboard: String?
    private var clipboardSession: String?
    private var sendingClipboard = false
    private let clipboardQueue = DispatchQueue(label: "com.vnilink.clipboard", qos: .userInitiated)
    private let transferQueue = DispatchQueue(label: "com.vnilink.transfer", qos: .userInitiated)
    private let jobWorker = DispatchQueue(label: "com.vnilink.jobs", qos: .userInitiated)
    private var drivingJobs = false
    @Published private(set) var sendingHostFile = false
    private var host: MacHost?
    private var cursorTimer: Timer?
    private var cursorSampler: HostCursorSampler?
    private var sendingCursor = false
    private let hostQueue = DispatchQueue(label: "com.vnilink.host", qos: .userInitiated)

    func setSharing(_ enabled: Bool, resumeHosting: Bool = false) {
        guard let core, !changingSharing, enabled != sharing else { return }
        if enabled, !CGPreflightScreenCaptureAccess() {
            if !CGRequestScreenCaptureAccess() {
                error = "Allow Screen Recording for VniLink in System Settings, then enable sharing again."
                return
            }
        }
        changingSharing = true
        let old = host
        let display = CGMainDisplayID(), bounds = CGDisplayBounds(CGMainDisplayID())
        let mode = CGDisplayCopyDisplayMode(display)
        let pixelWidth = mode?.pixelWidth ?? Int(bounds.width)
        let pixelHeight = mode?.pixelHeight ?? Int(bounds.height)
        // If the public system-cursor API is unavailable, retain embedded
        // ScreenCaptureKit cursors rather than negotiating an empty channel.
        let sampler = enabled ? HostCursorSampler() : nil
        let cursorSupported = sampler?.available == true
        hostQueue.async {
            let result = Result { () -> MacHost? in
                if enabled {
                    guard bounds.width >= 2, bounds.height >= 2, bounds.width <= 65535, bounds.height <= 65535, pixelWidth >= 2, pixelHeight >= 2, pixelWidth <= 65535, pixelHeight <= 65535 else {
                        throw FfiError.Unsupported(message: "This main display is not supported")
                    }
                    return try core.macHost(displayId: display, width: UInt16(bounds.width), height: UInt16(bounds.height), pixelWidth: UInt16(pixelWidth), pixelHeight: UInt16(pixelHeight), cursorSupported: cursorSupported)
                }
                old?.stop(); return nil
            }
            DispatchQueue.main.async {
                self.changingSharing = false
                switch result {
                case .success(let host):
                    self.host = host; self.sharing = enabled
                    self.startCursorUpdates(cursorSupported ? sampler : nil)
                    if enabled && resumeHosting { self.perform { try $0.startHosting() } }
                case .failure(let error): self.error = Self.message(error)
                }
            }
        }
    }
    private func startCursorUpdates(_ sampler: HostCursorSampler?) {
        cursorTimer?.invalidate(); cursorTimer = nil; cursorSampler = sampler
        guard sampler != nil else { return }
        let timer = Timer(timeInterval: 1.0 / 60, repeats: true) { [weak self] _ in
            // This timer is installed exclusively on the main run loop.
            MainActor.assumeIsolated {
                guard let self, !self.sendingCursor, let host = self.host,
                      let update = self.cursorSampler?.sample() else { return }
                self.sendingCursor = true
                self.hostQueue.async {
                    try? host.updateCursor(update: update)
                    DispatchQueue.main.async { self.sendingCursor = false }
                }
            }
        }
        RunLoop.main.add(timer, forMode: .common); cursorTimer = timer
    }
    func enableInput() {
        guard AXIsProcessTrusted() else {
            AXIsProcessTrustedWithOptions([kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String: true] as CFDictionary)
            error = "Allow Accessibility for VniLink in System Settings, then enable remote input again."
            return
        }
        perform({ try $0.enableMacInput() }, success: "Remote input enabled for this launch.", completed: { self.inputEnabled = true })
    }
    private var core: VnilinkCore?
    private var timer: Timer?
    private let worker = DispatchQueue(label: "com.vnilink.core", qos: .userInitiated)
    private var refreshing = false
    private var starting = false
    private var pendingFile: URL?
    @Published private(set) var viewerTabs: [ViewerModel] = []
    @Published var selectedTab: UUID?
    @Published private(set) var controllerSessions: [FfiSessionInfo] = []

    func showHome() { selectedTab = nil }
    func selectAdjacentTab(_ offset: Int) {
        let ids: [UUID?] = [nil] + viewerTabs.map { Optional($0.id) }
        let index = ids.firstIndex(of: selectedTab) ?? 0
        selectedTab = ids[(index + offset + ids.count) % ids.count]
    }
    func closeTab(_ id: UUID) {
        guard let index = viewerTabs.firstIndex(where: { $0.id == id }) else { return }
        let tab = viewerTabs.remove(at: index)
        tab.close()
        if selectedTab == id { selectedTab = index > 0 ? viewerTabs[index - 1].id : nil }
    }
    private func closeAllTabs() {
        for tab in viewerTabs { tab.close() }
        viewerTabs.removeAll(); selectedTab = nil
    }
    private var filesWindows: [String: FilesWindowController] = [:]

    func connect(_ device: FfiDeviceInfo) {
        guard let core else { return }
        if let existing = viewerTabs.first(where: { $0.deviceId == device.id }) {
            if existing.session?.phase == .closed || existing.session?.phase.isFailed == true || existing.failure != nil && existing.session == nil {
                closeTab(existing.id)
            } else {
                selectedTab = existing.id
                return
            }
        }
        let viewer = ViewerModel(core: core, device: device)
        viewerTabs.append(viewer); selectedTab = viewer.id
    }

    /// Opens Files without the desktop viewer and without host screen sharing:
    /// reuses the live control session for this peer when present, otherwise
    /// establishes one for file roots/listings/transfers only.
    func openFiles(_ device: FfiDeviceInfo) {
        guard let core else { return }
        if let existing = filesWindows[device.id], existing.window?.isVisible == true {
            existing.window?.makeKeyAndOrderFront(nil)
            return
        }
        // Establish the control session for files when none targets this peer.
        // Files-only: no video decoder or host screen sharing required
        // (Transfer grant suffices; Control not required).
        worker.async {
            let result = Result { () -> Void in
                let sessions = try core.controllerSessions()
                if !sessions.contains(where: { $0.peerId == device.id && $0.phase != .closed && !$0.phase.isFailed }) {
                    _ = try core.beginConnectFiles(endpointId: device.id)
                }
            }
            DispatchQueue.main.async {
                switch result {
                case .success: self.showFiles(device)
                case .failure(let error): self.error = Self.message(error)
                }
            }
        }
    }

    private func showFiles(_ device: FfiDeviceInfo) {
        guard let core else { return }
        let name = device.name
        let controller = FilesWindowController.open(
            core: core,
            session: { [weak self] in
                self?.controllerSessions.first { $0.phase == .active && $0.peerId == device.id }
            },
            peerId: device.id,
            localName: Host.current().localizedName ?? "This Mac",
            remoteName: name
        )
        filesWindows[device.id] = controller
        controller.showWindow(nil)
        controller.window?.makeKeyAndOrderFront(nil)
    }

    func start() {
        guard core == nil, !starting else { return }
        starting = true
        let previewMode = isPreview
        let clipboard = MacClipboard.shared, receiveFolder = receiveFolder
        let savedMode = UserDefaults.standard.integer(forKey: "keyboardMode")
        let savedKeyboardMode: FfiKeyboardMode = savedMode == 1 ? .map : savedMode == 2 ? .translate : .character
        let savedSwap = UserDefaults.standard.bool(forKey: "swapControlAndCommand")
        worker.async {
            let result = Result {
                if previewMode {
                    return try Self.previewCore()
                }
                let core = try VnilinkCore.openPersistent(store: KeychainStore())
                do {
                    try core.prepareMacVideo()
                    try core.installClipboard(writer: clipboard)
                    try core.setTransferDirectory(path: receiveFolder.path)
                    try core.setBlobDirectory(path: Self.blobDirectory.path)
                    try core.setFileRoots(paths: Self.fileRoots)
                    try core.setKeyboardMode(mode: savedKeyboardMode)
                    try core.setSwapControlAndCommand(enabled: savedSwap)
                } catch { core.shutdown(); throw error }
                return core
            }
            DispatchQueue.main.async {
                self.starting = false
                switch result {
                case .success(let core):
                    self.core = core
                    self.worker.async {
                        let events = try? core.subscribeEvents()
                        DispatchQueue.main.async { self.events = events }
                    }
                    if !previewMode {
                        let background = BackgroundSettings.shared
                        if background.resumeSharing {
                            if CGPreflightScreenCaptureAccess() { self.setSharing(true, resumeHosting: true) }
                            else { self.notice = "Screen sharing was not resumed. Allow Screen Recording, then enable sharing in This Mac." }
                        }
                        if background.resumeInput && AXIsProcessTrusted() { self.enableInput() }
                    }
                    if let url = self.pendingFile { self.pendingFile = nil; self.importFile(url) }
                    if previewMode {
                        self.notice = nil
                        if (ProcessInfo.processInfo.arguments.contains("--preview-tabs") || Bundle.main.bundleIdentifier == "com.vnilink.tabs-preview"), let device = try? core.snapshot().devices.first {
                            for name in ["Linux laptop", "Mac mini"] {
                                var preview = device; preview.petName = name
                                self.viewerTabs.append(ViewerModel(core: core, device: preview, preview: true))
                            }
                            self.selectedTab = self.viewerTabs.first?.id
                        }
                    }
                    self.refresh()
                    self.scheduleRefresh()
                case .failure(let error):
                    self.error = Self.message(error)
                    if (ProcessInfo.processInfo.arguments.contains("--preview") || Bundle.main.bundleIdentifier == "com.vnilink.tabs-preview") { NSLog("VniLink preview startup: %@", Self.message(error)) }
                }
            }
        }
    }
    private nonisolated static func previewCore() throws -> VnilinkCore {
        let core = try VnilinkCore.create(publicRelay: false, customRelays: ["http://127.0.0.1:9"])
        let other = try VnilinkCore.create(publicRelay: false, customRelays: ["http://127.0.0.1:9"])
        defer { other.shutdown() }
        try core.prepareMacVideo()
        let device = try core.acceptInvitation(token: other.createInvitation(ttlSecs: 900))
        try core.setPeerPetName(endpointId: device.endpointId, name: "Linux laptop")
        return core
    }
    #if DEBUG
    func loadPreviewDevices() {
        guard !sharing, !changingSharing, !starting,
              snapshot?.incoming == nil, controllerSessions.allSatisfy({ $0.phase == .closed || $0.phase.isFailed }) else {
            error = "Stop desktop sessions and screen sharing before opening the preview."; return
        }
        timer?.invalidate(); starting = true
        let previous = core
        worker.async {
            let result = Result { try Self.previewCore() }
            if case .success = result { previous?.shutdown() }
            DispatchQueue.main.async {
                self.starting = false
                switch result {
                case .success(let core):
                    self.core = core; self.isPreview = true; self.snapshot = nil; self.selectedDeviceID = nil; self.destination = .devices
                    self.notice = nil; self.refresh()
                case .failure(let error): self.error = Self.message(error)
                }
                self.scheduleRefresh()
            }
        }
    }
    #endif
    private func scheduleRefresh() {
        timer?.invalidate()
        timer = Timer.scheduledTimer(withTimeInterval: 0.7, repeats: true) { [weak self] _ in
            Task { @MainActor in self?.refresh() }
            self?.driveJobsInBackground()
        }
    }
    /// Advances transfers without any Files window open. Runs off the main
    /// thread; the core's per-job guard keeps this from double-advancing
    /// alongside open Files windows.
    private func driveJobsInBackground() {
        guard let core, !drivingJobs else { return }
        drivingJobs = true
        jobWorker.async {
            do {
                let jobs = try core.jobList()
                for job in jobs where ![.complete, .cancelled, .failed].contains(job.state) && (job.sender || job.state != .waitingDecision) {
                    _ = try? core.jobPoll(id: job.id)
                }
            } catch {}
            DispatchQueue.main.async { self.drivingJobs = false }
        }
    }
    func refresh() {
        guard let core, !refreshing else { return }
        refreshing = true
        let host = host
        let events = events
        let selectedId = viewerTabs.first { $0.id == selectedTab }?.session?.id
        worker.async {
            let result = Result { (try core.snapshot(), try core.controlRequest(), try core.savedRelays(), try core.keyboardMode(), try core.swapControlAndCommand(), host?.error(), events?.poll(maxEvents: 128) ?? [], selectedId.flatMap { id -> String? in
                guard let session = try? core.controllerSession(sessionId: id), session.phase == .active else { return nil }
                return try? core.sessionClipboardText(sessionId: id, generation: session.generation)
            }, try core.controllerSessions()) }
            DispatchQueue.main.async {
                self.refreshing = false
                switch result {
                case .success(let data):
                    self.controllerSessions = data.8
                    if self.snapshot != data.0 { self.snapshot = data.0 }
                    if self.request != data.1 { self.request = data.1 }
                    if self.savedRelays != data.2 { self.savedRelays = data.2 }
                    if self.keyboardMode != data.3 { self.keyboardMode = data.3 }
                    if self.swapModifiers != data.4 { self.swapModifiers = data.4 }
                    if let error = data.5, self.sharing, !self.changingSharing {
                        self.setSharing(false); self.error = error
                    }
                    if let selected = self.selectedDeviceID, !data.0.devices.contains(where: { $0.id == selected }) { self.selectedDeviceID = nil }
                    let clipboardReceived = data.6.contains { event in
                        if case .clipboardReceived(let id) = event { return id == selectedId }
                        return false
                    }
                    if selectedId == self.viewerTabs.first(where: { $0.id == self.selectedTab })?.session?.id {
                        self.syncClipboard(core: core, snapshot: data.0, outgoing: data.8.first { $0.id == selectedId && $0.phase == .active }, remote: data.7, received: clipboardReceived)
                    }
                    for event in data.6 {
                        if case .transferCompleted = event { self.notice = "File received in \(self.receiveFolder.lastPathComponent)." }
                    }
                case .failure(let error): self.error = Self.message(error)
                }
            }
        }
    }
    private func syncClipboard(core: VnilinkCore, snapshot: FfiSnapshot, outgoing: FfiSessionInfo?, remote: String?, received: Bool) {
        let incoming = snapshot.incoming.flatMap { session -> FfiSessionInfo? in
            guard session.phase == .active,
                  snapshot.devices.contains(where: { $0.endpointId == session.peerId && $0.grants.clipboard }) else { return nil }
            return session
        }
        let stamp = outgoing.map { "out:\($0.id):\($0.generation)" } ?? incoming.map { "in:\($0.id):\($0.generation)" }
        if clipboardSession != stamp {
            clipboardSession = stamp; lastRemoteClipboard = nil
            MacClipboard.shared.baseline()
        }
        guard stamp != nil else { return }
        // Apply incoming host text before inspecting local changes, avoiding an echo.
        if outgoing != nil, let remote, received || remote != lastRemoteClipboard {
            try? MacClipboard.shared.setText(text: remote)
            lastRemoteClipboard = remote
        }
        guard !sendingClipboard, let text = MacClipboard.shared.localChange(), text.utf8.count <= 256 * 1024 else { return }
        sendingClipboard = true
        clipboardQueue.async {
            if let outgoing { try? core.submitClipboard(sessionId: outgoing.id, generation: outgoing.generation, text: text) }
            if let incoming { try? core.publishClipboard(sessionId: incoming.id, text: text) }
            DispatchQueue.main.async { self.sendingClipboard = false }
        }
    }
    func chooseReceiveFolder() {
        let panel = NSOpenPanel(); panel.canChooseFiles = false; panel.canChooseDirectories = true; panel.allowsMultipleSelection = false
        if panel.runModal() == .OK, let url = panel.url {
            perform({ try $0.setTransferDirectory(path: url.path) }, success: "Receive folder saved.", completed: {
                self.receiveFolder = url; UserDefaults.standard.set(url, forKey: "receiveFolder")
            })
        }
    }
    func chooseFileRoots() {
        let panel = NSOpenPanel(); panel.canChooseFiles = false; panel.canChooseDirectories = true; panel.allowsMultipleSelection = true
        panel.message = "Folders the Files window may browse on this Mac. Paired computers see only these folders."
        if panel.runModal() == .OK, !panel.urls.isEmpty {
            let paths = panel.urls.map(\.path)
            perform({ try $0.setFileRoots(paths: paths) }, success: "Shared folders saved.", completed: {
                self.fileRoots = paths; UserDefaults.standard.set(paths, forKey: "fileRoots")
            })
        }
    }
    func setKeyboardMode(_ mode: FfiKeyboardMode) {
        perform({ try $0.setKeyboardMode(mode: mode) }, completed: {
            UserDefaults.standard.set(mode == .map ? 1 : mode == .translate ? 2 : 0, forKey: "keyboardMode")
        })
    }
    func setModifierSwap(_ enabled: Bool) {
        perform({ try $0.setSwapControlAndCommand(enabled: enabled) }, completed: {
            UserDefaults.standard.set(enabled, forKey: "swapControlAndCommand")
        })
    }
    func sendHostFile() {
        guard let core, let session = snapshot?.incoming, session.phase == .active, !sendingHostFile else { return }
        let panel = NSOpenPanel(); panel.canChooseDirectories = false; panel.allowsMultipleSelection = false
        guard panel.runModal() == .OK, let url = panel.url else { return }
        sendingHostFile = true; notice = "Sending \(url.lastPathComponent)…"
        transferQueue.async {
            let result = Result { try core.publishFile(sessionId: session.id, name: url.lastPathComponent,
                bytes: DesktopFiles.read(url), replace: false) }
            DispatchQueue.main.async {
                self.sendingHostFile = false
                switch result {
                case .success: self.notice = "\(url.lastPathComponent) received by the connected computer."
                case .failure(let error): self.error = Self.message(error)
                }
            }
        }
    }
    func perform(_ work: @escaping (VnilinkCore) throws -> Void, success: String? = nil, completed: (() -> Void)? = nil) {
        guard let core else { return }
        worker.async {
            let result = Result { try work(core) }
            DispatchQueue.main.async {
                switch result {
                case .success: self.notice = success; completed?(); self.refresh()
                case .failure(let error): self.error = Self.message(error)
                }
            }
        }
    }
    func importFile(_ url: URL) {
        guard let core else { pendingFile = url; return }
        worker.async {
            let result = Result {
                let values = try url.resourceValues(forKeys: [.fileSizeKey])
                guard (values.fileSize ?? 0) <= 20_000 else { throw FfiError.InvalidPairingFile }
                return try core.openPairingFile(bytes: Data(contentsOf: url))
            }
            DispatchQueue.main.async {
                switch result {
                case .success(let preview): self.adding = false; self.preview = preview
                case .failure(let error): self.error = Self.message(error)
                }
            }
        }
    }
    func choosePairingFile() {
        let panel = NSOpenPanel(); panel.allowedContentTypes = [.pairing]; panel.allowsMultipleSelection = false
        if panel.runModal() == .OK, let url = panel.url { importFile(url) }
    }
    func confirmPairing(_ digits: String) {
        guard let preview, let core else { return }
        worker.async {
            let result = Result { try core.confirmPairing(previewId: preview.id, digits: digits) }
            DispatchQueue.main.async {
                switch result {
                case .success(let device): self.preview = nil; self.destination = .devices; self.selectedDeviceID = device.id; self.refresh()
                case .failure(let error): self.error = Self.message(error)
                }
            }
        }
    }
    func createPairingFile() {
        guard let core else { return }
        worker.async {
            let result = Result { try core.exportPairingFile(ttlSecs: 900, allowControl: true) }
            DispatchQueue.main.async {
                switch result {
                case .success(let file):
                    let panel = NSSavePanel(); panel.allowedContentTypes = [.pairing]; panel.nameFieldStringValue = "This computer.vnl"
                    if panel.runModal() == .OK, let url = panel.url {
                        do { try file.bytes.write(to: url, options: .atomic); self.offerCode = file.check }
                        catch { self.error = "Could not save the pairing file." }
                    }
                case .failure(let error): self.error = Self.message(error)
                }
            }
        }
    }
    func acceptToken(_ token: String, completed: (() -> Void)? = nil) {
        guard let core else { return }
        worker.async {
            let result = Result { try core.acceptInvitation(token: token) }
            DispatchQueue.main.async {
                switch result {
                case .success(let device):
                    self.adding = false; self.destination = .devices; self.selectedDeviceID = device.id
                    self.notice = "Device added."; completed?(); self.refresh()
                case .failure(let error): self.error = Self.message(error)
                }
            }
        }
    }
    func copyInvitation() {
        guard let core else { return }
        worker.async {
            let result = Result { try core.createInvitation(ttlSecs: 900) }
            DispatchQueue.main.async {
                switch result {
                case .success(let token):
                    NSPasteboard.general.clearContents(); NSPasteboard.general.setString(token, forType: .string)
                    self.notice = "Invitation copied. It expires in 15 minutes."
                case .failure(let error): self.error = Self.message(error)
                }
            }
        }
    }
    func resetIdentity() {
        guard let core else { return }
        guard !changingSharing else { error = "Wait for screen sharing to finish updating, then reset the identity."; return }
        closeAllTabs()
        startCursorUpdates(nil)
        let oldHost = host; host = nil; sharing = false; inputEnabled = false
        hostQueue.async { oldHost?.stop() }
        worker.async {
            let result = Result { try core.resetIdentity() }
            DispatchQueue.main.async {
                switch result {
                case .success:
                    self.timer?.invalidate(); self.core = nil; self.snapshot = nil
                    self.events = nil; self.clipboardSession = nil; self.lastRemoteClipboard = nil
                    self.destination = .devices; self.selectedDeviceID = nil; self.request = nil; self.preview = nil
                    self.notice = "Identity reset. Pair your computers again."
                    self.start()
                case .failure(let error): self.error = Self.message(error)
                }
            }
        }
    }
    func shutdown(_ completion: @escaping () -> Void) {
        startCursorUpdates(nil)
        timer?.invalidate()
        closeAllTabs()
        let core = core, host = host
        hostQueue.async {
            host?.stop()
            self.worker.async { core?.shutdown(); DispatchQueue.main.async(execute: completion) }
        }
    }
    static func message(_ error: Error) -> String {
        switch error {
        case FfiError.StoreLocked: return "The login keychain is locked. Unlock it, then try again."
        case FfiError.StoreUnavailable: return "VniLink could not access the login keychain."
        case FfiError.PairingCheckMismatch: return "The code does not match. Check the other computer."
        case FfiError.InvalidInvitation: return "This invitation is not valid."
        case FfiError.InvitationExpired: return "This invitation has expired. Create a new one."
        case FfiError.ClipboardDenied: return "Allow Clipboard text for this computer in the other computer's device permissions."
        case FfiError.TransferDenied: return "Allow File transfer for this computer in the other computer's device permissions."
        case FfiError.FileExists: return "A file with this name is already in the receive folder. Rename the file before sending it."
        case FfiError.InputFailed(let message), FfiError.Unsupported(let message), FfiError.InvalidSetting(let message): return message
        default: return error.localizedDescription
        }
    }
}

/// AppKit cursor images are sampled on the main thread. Only changed shapes
/// cross FFI; position samples are independent of capture and video decoding.
@MainActor private final class HostCursorSampler {
    let available: Bool
    private var previousPixels: Data?
    private var previousHotspot: NSPoint?
    private var previousSize: NSSize?

    init() { available = NSCursor.currentSystem != nil }

    func sample() -> MacCursorUpdate? {
        guard let cursor = NSCursor.currentSystem,
              let location = CGEvent(source: nil)?.location else { return nil }
        let image = cursor.image
        let width = Int(image.size.width.rounded(.up)), height = Int(image.size.height.rounded(.up))
        guard width > 0, height > 0, width <= 512, height <= 512,
              let source = image.cgImage(forProposedRect: nil, context: nil, hints: nil) else { return nil }
        var pixels = Data(count: width * height * 4)
        let rendered = pixels.withUnsafeMutableBytes { bytes -> Bool in
            guard let base = bytes.baseAddress,
                  let context = CGContext(data: base, width: width, height: height,
                      bitsPerComponent: 8, bytesPerRow: width * 4,
                      space: CGColorSpaceCreateDeviceRGB(),
                      bitmapInfo: CGBitmapInfo.byteOrder32Little.rawValue | CGImageAlphaInfo.premultipliedFirst.rawValue) else { return false }
            context.draw(source, in: CGRect(x: 0, y: 0, width: width, height: height))
            return true
        }
        guard rendered else { return nil }
        var shape: MacCursorImage?
        if pixels != previousPixels || cursor.hotSpot != previousHotspot || image.size != previousSize {
            shape = MacCursorImage(width: UInt16(width), height: UInt16(height),
                hotspotX: UInt16(max(0, min(width - 1, Int(cursor.hotSpot.x)))),
                hotspotY: UInt16(max(0, min(height - 1, Int(cursor.hotSpot.y)))), bgra: pixels)
            previousPixels = pixels; previousHotspot = cursor.hotSpot; previousSize = image.size
        }
        return MacCursorUpdate(x: Int32(location.x.rounded()), y: Int32(location.y.rounded()),
            visible: true, image: shape)
    }
}

extension FfiSessionPhase {
    var isFailed: Bool { if case .failed = self { return true }; return false }
}
