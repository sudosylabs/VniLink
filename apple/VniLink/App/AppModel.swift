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
    @Published private(set) var isPreview = ProcessInfo.processInfo.arguments.contains("--preview")
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
    private var host: MacHost?
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
        hostQueue.async {
            let result = Result { () -> MacHost? in
                if enabled {
                    guard bounds.width >= 2, bounds.height >= 2, bounds.width <= 65535, bounds.height <= 65535 else {
                        throw FfiError.Unsupported(message: "This main display is not supported")
                    }
                    return try core.macHost(displayId: display, width: UInt16(bounds.width), height: UInt16(bounds.height))
                }
                old?.stop(); return nil
            }
            DispatchQueue.main.async {
                self.changingSharing = false
                switch result {
                case .success(let host):
                    self.host = host; self.sharing = enabled
                    if enabled && resumeHosting { self.perform { try $0.startHosting() } }
                case .failure(let error): self.error = Self.message(error)
                }
            }
        }
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
    private var viewerWindow: ViewerWindowController?

    func connect(_ device: FfiDeviceInfo) {
        guard let core else { return }
        if let viewerWindow, viewerWindow.window?.isVisible == true {
            viewerWindow.window?.makeKeyAndOrderFront(nil)
            return
        }
        viewerWindow = ViewerWindowController(core: core, device: device)
        viewerWindow?.showWindow(nil)
        viewerWindow?.window?.makeKeyAndOrderFront(nil)
    }

    func start() {
        guard core == nil, !starting else { return }
        starting = true
        let previewMode = isPreview
        worker.async {
            let result = Result {
                if previewMode {
                    return try Self.previewCore()
                }
                let core = try VnilinkCore.openPersistent(store: KeychainStore())
                do { try core.prepareMacVideo() } catch { core.shutdown(); throw error }
                return core
            }
            DispatchQueue.main.async {
                self.starting = false
                switch result {
                case .success(let core):
                    self.core = core
                    if !previewMode {
                        let background = BackgroundSettings.shared
                        if background.resumeSharing {
                            if CGPreflightScreenCaptureAccess() { self.setSharing(true, resumeHosting: true) }
                            else { self.notice = "Screen sharing was not resumed. Allow Screen Recording, then enable sharing in This Mac." }
                        }
                        if background.resumeInput && AXIsProcessTrusted() { self.enableInput() }
                    }
                    if let url = self.pendingFile { self.pendingFile = nil; self.importFile(url) }
                    if previewMode { self.notice = nil }
                    self.refresh()
                    self.scheduleRefresh()
                case .failure(let error):
                    self.error = Self.message(error)
                    if ProcessInfo.processInfo.arguments.contains("--preview") { NSLog("VniLink preview startup: %@", Self.message(error)) }
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
              snapshot?.incoming == nil, snapshot?.outgoing == nil else {
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
        }
    }
    func refresh() {
        guard let core, !refreshing else { return }
        refreshing = true
        let host = host
        worker.async {
            let result = Result { (try core.snapshot(), try core.controlRequest(), try core.savedRelays(), try core.keyboardMode(), try core.swapControlAndCommand(), host?.error()) }
            DispatchQueue.main.async {
                self.refreshing = false
                switch result {
                case .success(let data):
                    if self.snapshot != data.0 { self.snapshot = data.0 }
                    if self.request != data.1 { self.request = data.1 }
                    if self.savedRelays != data.2 { self.savedRelays = data.2 }
                    if self.keyboardMode != data.3 { self.keyboardMode = data.3 }
                    if self.swapModifiers != data.4 { self.swapModifiers = data.4 }
                    if let error = data.5, self.sharing, !self.changingSharing {
                        self.setSharing(false); self.error = error
                    }
                    if let selected = self.selectedDeviceID, !data.0.devices.contains(where: { $0.id == selected }) { self.selectedDeviceID = nil }
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
        viewerWindow?.close()
        let oldHost = host; host = nil; sharing = false; inputEnabled = false
        hostQueue.async { oldHost?.stop() }
        worker.async {
            let result = Result { try core.resetIdentity() }
            DispatchQueue.main.async {
                switch result {
                case .success:
                    self.timer?.invalidate(); self.core = nil; self.snapshot = nil
                    self.destination = .devices; self.selectedDeviceID = nil; self.request = nil; self.preview = nil
                    self.notice = "Identity reset. Pair your computers again."
                    self.start()
                case .failure(let error): self.error = Self.message(error)
                }
            }
        }
    }
    func shutdown(_ completion: @escaping () -> Void) {
        timer?.invalidate()
        viewerWindow?.model.close()
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
        default: return String(describing: error)
        }
    }
}
