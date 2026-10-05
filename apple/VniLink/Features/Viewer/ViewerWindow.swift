import AppKit
import SwiftUI
import VnilinkCore

@MainActor final class ViewerModel: ObservableObject {
    @Published var session: FfiSessionInfo?
    @Published var picture: CGImage?
    @Published var status = "Connecting…"
    @Published var failure: String?
    @Published var code: String?
    @Published var controlling = true
    @Published var notice: String?
    let name: String
    private let core: VnilinkCore
    private let videoQueue = DispatchQueue(label: "com.vnilink.viewer.video", qos: .userInitiated)
    private let inputQueue = DispatchQueue(label: "com.vnilink.viewer.input", qos: .userInteractive)
    private let connectionQueue = DispatchQueue(label: "com.vnilink.viewer.connection", qos: .userInitiated)
    private var timer: Timer?
    private var poller: MacViewer?
    private var updating = false
    private var closed = false
    private var epoch: UInt64 = 0
    private var decodingGeneration: UInt64?
    private var pendingInput = 0

    init(core: VnilinkCore, device: FfiDeviceInfo) {
        self.core = core; name = device.name
        timer = Timer.scheduledTimer(withTimeInterval: 0.1, repeats: true) { [weak self] _ in Task { @MainActor in self?.update() } }
        connectionQueue.async {
            let result = Result { try core.connect(endpointId: device.id) }
            DispatchQueue.main.async {
                guard !self.closed else { return }
                if case .failure(let error) = result { self.failure = AppModel.message(error); self.status = "Connection failed" }
            }
        }
    }
    private func update() {
        guard !closed, !updating else { return }
        updating = true
        let epoch = epoch
        let poller = poller
        videoQueue.async {
            let result = Result { (try self.core.snapshot().outgoing, self.core.controlCheck(), poller?.poll(), poller?.error()) }
            DispatchQueue.main.async {
                self.updating = false
                guard !self.closed, self.epoch == epoch else { return }
                switch result {
                case .failure(let error): self.failure = AppModel.message(error)
                case .success(let data):
                    self.session = data.0; self.code = data.0?.phase == .active ? nil : data.1
                    if let error = data.3 { self.failure = error }
                    guard let session = data.0 else { return }
                    self.status = Self.label(session.phase)
                    if session.phase == .active {
                        if self.decodingGeneration != session.generation {
                            self.startDecoder(session)
                        } else if let frame = data.2 { self.picture = Self.image(frame) }
                    } else {
                        self.picture = nil
                        if self.poller != nil { self.stopDecoder() }
                        if case .failed(let message) = session.phase { self.failure = message }
                    }
                }
            }
        }
    }
    private func startDecoder(_ session: FfiSessionInfo) {
        stopDecoder()
        decodingGeneration = session.generation
        let epoch = epoch
        videoQueue.async {
            let result = Result { try self.core.macViewer(sessionId: session.id, generation: session.generation) }
            DispatchQueue.main.async {
                guard !self.closed, epoch == self.epoch else {
                    if case .success(let poller) = result { self.videoQueue.async { poller.stop() } }
                    return
                }
                switch result {
                case .success(let poller): self.poller = poller
                case .failure(let error): self.failure = AppModel.message(error)
                }
            }
        }
    }
    private func stopDecoder() {
        epoch += 1
        let old = poller; poller = nil; decodingGeneration = nil; picture = nil
        videoQueue.async { old?.stop() }
    }
    func send(release: Bool = false, _ operation: @escaping (VnilinkCore, FfiSessionInfo) throws -> Void) {
        guard !closed, (controlling || release), let session, session.phase == .active else { return }
        guard pendingInput < 128 else { failure = "Input fell behind. Reconnect to continue."; close(); return }
        pendingInput += 1
        inputQueue.async {
            let result = Result { try operation(self.core, session) }
            DispatchQueue.main.async {
                self.pendingInput -= 1
                if case .failure(let error) = result, !self.closed {
                    self.failure = AppModel.message(error); self.close()
                }
            }
        }
    }
    func quality(_ bitrate: UInt32, _ frameRate: UInt8, _ width: UInt16, _ height: UInt16) {
        guard let session, session.phase == .active else { return }
        action { core in
            try core.updateSessionQuality(sessionId: session.id, role: .controller,
                quality: FfiQuality(maxBitrateKbps: bitrate, maxFrameRate: frameRate, maxWidth: width, maxHeight: height))
        }
    }
    private func action(_ operation: @escaping (VnilinkCore) throws -> Void, success: String? = nil) {
        connectionQueue.async {
            let result = Result { try operation(self.core) }
            DispatchQueue.main.async {
                guard !self.closed else { return }
                if case .failure(let error) = result { self.notice = AppModel.message(error) }
                else { self.notice = success }
            }
        }
    }
    func sendClipboard() {
        guard let session, session.phase == .active, let text = NSPasteboard.general.string(forType: .string) else { return }
        action({ try $0.submitClipboard(sessionId: session.id, generation: session.generation, text: text) }, success: "Clipboard text sent.")
    }
    func sendFile() {
        guard let session, session.phase == .active else { return }
        let panel = NSOpenPanel(); panel.canChooseDirectories = false; panel.allowsMultipleSelection = false
        if panel.runModal() == .OK, let url = panel.url {
            action({ core in
                let size = try url.resourceValues(forKeys: [.fileSizeKey]).fileSize ?? 0
                guard size <= 8 * 1024 * 1024 else { throw FfiError.InvalidSetting(message: "Files must be 8 MiB or smaller") }
                try core.submitFile(sessionId: session.id, generation: session.generation, name: url.lastPathComponent,
                                    bytes: Data(contentsOf: url), replace: false)
            }, success: "File sent.")
        }
    }
    func close() {
        guard !closed else { return }
        closed = true; timer?.invalidate(); stopDecoder(); status = "Disconnected"; code = nil; session = nil
        // Separate from connect's blocking worker so Cancel works during approval.
        inputQueue.async { try? self.core.disconnect() }
    }
    static func image(_ frame: MacFrame) -> CGImage? {
        let data = frame.bgra as CFData
        guard let provider = CGDataProvider(data: data) else { return nil }
        return CGImage(width: Int(frame.width), height: Int(frame.height), bitsPerComponent: 8, bitsPerPixel: 32,
                       bytesPerRow: Int(frame.stride), space: CGColorSpaceCreateDeviceRGB(),
                       bitmapInfo: [.byteOrder32Little, CGBitmapInfo(rawValue: CGImageAlphaInfo.noneSkipFirst.rawValue)],
                       provider: provider, decode: nil, shouldInterpolate: true, intent: .defaultIntent)
    }
    static func label(_ phase: FfiSessionPhase) -> String {
        switch phase {
        case .connecting: return "Connecting…"
        case .authenticated, .authorized, .negotiating: return "Waiting for the other computer…"
        case .active: return "Connected"
        case .reconnecting: return "Reconnecting…"
        case .stopping: return "Disconnecting…"
        case .closed: return "Disconnected"
        case .failed: return "Connection failed"
        }
    }
}

@MainActor final class ViewerWindowController: NSWindowController, NSWindowDelegate, NSToolbarDelegate {
    let model: ViewerModel
    init(core: VnilinkCore, device: FfiDeviceInfo) {
        model = ViewerModel(core: core, device: device)
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1100, height: 740),
                              styleMask: [.titled, .closable, .miniaturizable, .resizable], backing: .buffered, defer: false)
        window.title = device.name
        window.minSize = NSSize(width: 640, height: 420)
        window.contentView = NSHostingView(rootView: ViewerContent(model: model))
        window.isReleasedWhenClosed = false
        window.collectionBehavior = [.fullScreenPrimary]
        super.init(window: window)
        window.delegate = self
        let toolbar = NSToolbar(identifier: "Desktop")
        toolbar.delegate = self; toolbar.displayMode = .iconOnly
        window.toolbar = toolbar; window.toolbarStyle = .unified
        window.center()
    }
    func toolbarAllowedItemIdentifiers(_ toolbar: NSToolbar) -> [NSToolbarItem.Identifier] { toolbarDefaultItemIdentifiers(toolbar) }
    func toolbarDefaultItemIdentifiers(_ toolbar: NSToolbar) -> [NSToolbarItem.Identifier] {
        [.init("quality"), .init("clipboard"), .init("file"), .flexibleSpace, .init("fullscreen"), .init("disconnect")]
    }
    func toolbar(_ toolbar: NSToolbar, itemForItemIdentifier identifier: NSToolbarItem.Identifier, willBeInsertedIntoToolbar flag: Bool) -> NSToolbarItem? {
        let item = NSToolbarItem(itemIdentifier: identifier)
        item.target = self
        switch identifier.rawValue {
        case "quality":
            let popup = NSPopUpButton(frame: .zero, pullsDown: true)
            popup.addItem(withTitle: "Picture quality")
            for (title, tag) in [("Low bandwidth", 0), ("Balanced", 1), ("Sharper picture", 2)] {
                popup.addItem(withTitle: title); popup.lastItem?.tag = tag
                popup.lastItem?.target = self; popup.lastItem?.action = #selector(changeQuality(_:))
            }
            item.view = popup; item.label = "Picture quality"
        case "clipboard": item.label = "Send clipboard text"; item.image = NSImage(systemSymbolName: "doc.on.clipboard", accessibilityDescription: item.label); item.action = #selector(sendClipboard)
        case "file": item.label = "Send file"; item.image = NSImage(systemSymbolName: "document.badge.arrow.up", accessibilityDescription: item.label); item.action = #selector(sendFile)
        case "fullscreen": item.label = "Full screen"; item.image = NSImage(systemSymbolName: "arrow.up.left.and.arrow.down.right", accessibilityDescription: item.label); item.action = #selector(fullscreen)
        case "disconnect": item.label = "Disconnect"; item.image = NSImage(systemSymbolName: "xmark.circle", accessibilityDescription: item.label); item.action = #selector(disconnect)
        default: return nil
        }
        item.toolTip = item.label
        return item
    }
    @objc private func changeQuality(_ item: NSMenuItem) {
        switch item.tag {
        case 0: model.quality(1_500, 15, 1280, 720)
        case 2: model.quality(12_000, 60, 2560, 1440)
        default: model.quality(5_000, 30, 1920, 1080)
        }
    }
    @objc private func sendClipboard() { model.sendClipboard() }
    @objc private func sendFile() { model.sendFile() }
    @objc private func fullscreen() { window?.toggleFullScreen(nil) }
    @objc private func disconnect() { model.close() }
    required init?(coder: NSCoder) { fatalError("init(coder:) is unavailable") }
    func windowWillClose(_ notification: Notification) { model.close() }
}

struct ViewerContent: View {
    @ObservedObject var model: ViewerModel
    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 18) {
                Text(model.status).font(.callout).foregroundStyle(.secondary)
                if let notice = model.notice { Text(notice).font(.caption).foregroundStyle(.secondary).lineLimit(2) }
                Spacer()
                Toggle("Control", isOn: $model.controlling).toggleStyle(.checkbox).disabled(model.session?.phase != .active)
            }.padding(.horizontal, 20).padding(.vertical, 12)
            Divider()
            ZStack {
                Color(nsColor: .underPageBackgroundColor)
                if let picture = model.picture {
                    DesktopCanvas(model: model, image: picture)
                } else {
                    VStack(spacing: 16) {
                        if model.failure == nil && model.status != "Disconnected" { ProgressView() }
                        Text(model.failure ?? model.status).font(.headline).multilineTextAlignment(.center)
                        if let code = model.code {
                            Text("Enter this code on \(model.name) to approve desktop access.").foregroundStyle(.secondary)
                            Text(code).font(.system(size: 40, weight: .medium, design: .monospaced)).tracking(8).textSelection(.enabled)
                        }
                    }.padding(36).frame(maxWidth: 520)
                }
            }
        }
    }
}

struct DesktopCanvas: NSViewRepresentable {
    let model: ViewerModel
    let image: CGImage
    func makeNSView(context: Context) -> DesktopView { DesktopView(model: model) }
    func updateNSView(_ view: DesktopView, context: Context) {
        view.image = image; view.controlling = model.controlling; view.needsDisplay = true
    }
}

final class DesktopView: NSView {
    private let model: ViewerModel
    var image: CGImage?
    var controlling = true { didSet { if !controlling { releaseInput() } } }
    private var keys = Set<UInt16>()
    private var buttons = Set<Int>()
    private var tracking: NSTrackingArea?
    private var inactiveObserver: NSObjectProtocol?
    init(model: ViewerModel) { self.model = model; super.init(frame: .zero) }
    required init?(coder: NSCoder) { fatalError("init(coder:) is unavailable") }
    override var acceptsFirstResponder: Bool { true }
    override var isFlipped: Bool { true }
    var imageRect: NSRect {
        guard let image else { return .zero }
        let scale = min(bounds.width / CGFloat(image.width), bounds.height / CGFloat(image.height))
        let size = NSSize(width: CGFloat(image.width) * scale, height: CGFloat(image.height) * scale)
        return NSRect(x: (bounds.width-size.width)/2, y: (bounds.height-size.height)/2, width: size.width, height: size.height)
    }
    override func draw(_ dirtyRect: NSRect) {
        guard let image, let context = NSGraphicsContext.current?.cgContext else { return }
        let rect = imageRect
        context.saveGState(); context.translateBy(x: rect.minX, y: rect.maxY); context.scaleBy(x: 1, y: -1)
        context.draw(image, in: CGRect(origin: .zero, size: rect.size)); context.restoreGState()
    }
    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let tracking { removeTrackingArea(tracking) }
        tracking = NSTrackingArea(rect: .zero, options: [.mouseMoved, .activeInKeyWindow, .inVisibleRect], owner: self)
        addTrackingArea(tracking!)
    }
    override func resignFirstResponder() -> Bool { releaseInput(); return super.resignFirstResponder() }
    override func viewWillMove(toWindow newWindow: NSWindow?) {
        releaseInput()
        if let inactiveObserver { NotificationCenter.default.removeObserver(inactiveObserver) }
        inactiveObserver = nil
        if let newWindow {
            inactiveObserver = NotificationCenter.default.addObserver(forName: NSWindow.didResignKeyNotification, object: newWindow, queue: .main) { [weak self] _ in self?.releaseInput() }
        }
        super.viewWillMove(toWindow: newWindow)
    }
    deinit { if let inactiveObserver { NotificationCenter.default.removeObserver(inactiveObserver) } }
    private func pointer(_ event: NSEvent) -> Bool {
        guard controlling, let session = model.session, session.monitorWidth > 0, session.monitorHeight > 0 else { return false }
        let point = convert(event.locationInWindow, from: nil), rect = imageRect
        guard rect.width > 0, rect.height > 0, rect.contains(point) else { return false }
        let x = UInt16(min(CGFloat(session.monitorWidth-1), max(0, (point.x-rect.minX)/rect.width * CGFloat(session.monitorWidth))))
        let y = UInt16(min(CGFloat(session.monitorHeight-1), max(0, (point.y-rect.minY)/rect.height * CGFloat(session.monitorHeight))))
        model.send { try $0.submitPointer(sessionId: $1.id, generation: $1.generation, x: x, y: y) }
        return true
    }
    override func mouseMoved(with event: NSEvent) { _ = pointer(event) }
    override func mouseDragged(with event: NSEvent) { _ = pointer(event) }
    override func rightMouseDragged(with event: NSEvent) { _ = pointer(event) }
    override func otherMouseDragged(with event: NSEvent) { _ = pointer(event) }
    private func button(_ event: NSEvent, down: Bool) {
        guard event.buttonNumber <= 2 else { return }
        if down {
            guard pointer(event) else { return }
            window?.makeFirstResponder(self); buttons.insert(event.buttonNumber)
        } else { guard buttons.remove(event.buttonNumber) != nil else { return } }
        let button: FfiPointerButton = event.buttonNumber == 0 ? .left : event.buttonNumber == 1 ? .right : .middle
        model.send { try $0.submitButton(sessionId: $1.id, generation: $1.generation, button: button, down: down) }
    }
    override func mouseDown(with event: NSEvent) { button(event, down: true) }
    override func mouseUp(with event: NSEvent) { button(event, down: false) }
    override func rightMouseDown(with event: NSEvent) { button(event, down: true) }
    override func rightMouseUp(with event: NSEvent) { button(event, down: false) }
    override func otherMouseDown(with event: NSEvent) { button(event, down: true) }
    override func otherMouseUp(with event: NSEvent) { button(event, down: false) }
    override func scrollWheel(with event: NSEvent) {
        guard pointer(event) else { return }
        let dx = Int16(max(-32767, min(32767, event.scrollingDeltaX.rounded())))
        let dy = Int16(max(-32767, min(32767, event.scrollingDeltaY.rounded())))
        model.send { try $0.submitWheel(sessionId: $1.id, generation: $1.generation, dx: dx, dy: dy) }
    }
    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        guard controlling, window?.firstResponder === self, event.modifierFlags.contains(.command) else { return false }
        keyDown(with: event); return true
    }
    override func keyDown(with event: NSEvent) {
        guard controlling, let usage = MacKeys.usage[event.keyCode] else { return }
        keys.insert(usage)
        let text = event.characters ?? ""
        model.send { try $0.submitLocalKey(sessionId: $1.id, generation: $1.generation, usage: usage, down: true, text: text) }
    }
    override func keyUp(with event: NSEvent) {
        guard let usage = MacKeys.usage[event.keyCode], keys.remove(usage) != nil else { return }
        model.send { try $0.submitLocalKey(sessionId: $1.id, generation: $1.generation, usage: usage, down: false, text: "") }
    }
    override func flagsChanged(with event: NSEvent) {
        guard controlling, let usage = MacKeys.usage[event.keyCode], let mask = MacKeys.modifiers[event.keyCode] else { return }
        let down = event.keyCode == 57 ? event.modifierFlags.contains(mask) : !keys.contains(usage)
        if down { keys.insert(usage) } else { keys.remove(usage) }
        model.send { try $0.submitKey(sessionId: $1.id, generation: $1.generation, usage: usage, down: down) }
    }
    private func releaseInput() {
        for usage in keys { model.send(release: true) { try $0.submitKey(sessionId: $1.id, generation: $1.generation, usage: usage, down: false) } }
        for number in buttons {
            let button: FfiPointerButton = number == 0 ? .left : number == 1 ? .right : .middle
            model.send(release: true) { try $0.submitButton(sessionId: $1.id, generation: $1.generation, button: button, down: false) }
        }
        keys.removeAll(); buttons.removeAll()
    }
}

// macOS hardware virtual-key codes to USB HID keyboard usages.
enum MacKeys {
    static let usage: [UInt16: UInt16] = [
        0:4,1:22,2:7,3:9,4:11,5:10,6:29,7:27,8:6,9:25,11:5,
        12:20,13:26,14:8,15:21,16:28,17:23,18:30,19:31,20:32,21:33,
        22:35,23:34,24:46,25:38,26:36,27:45,28:37,29:39,30:48,31:18,
        32:24,33:47,34:12,35:19,36:40,37:15,38:13,39:52,40:14,41:51,
        42:49,43:54,44:56,45:17,46:16,47:55,48:43,49:44,50:53,51:42,
        53:41,54:231,55:227,56:225,57:57,58:226,59:224,60:229,61:230,62:228,
        65:99,67:85,69:87,71:83,75:84,76:88,78:86,81:103,
        82:98,83:89,84:90,85:91,86:92,87:93,88:94,89:95,91:96,92:97,
        96:62,97:63,98:64,99:60,100:65,101:66,103:68,105:70,107:71,
        109:67,111:69,114:73,115:74,116:75,117:76,118:61,119:77,120:59,
        121:78,122:58,123:80,124:79,125:81,126:82
    ]
    static let modifiers: [UInt16: NSEvent.ModifierFlags] = [54:.command,55:.command,56:.shift,60:.shift,58:.option,61:.option,59:.control,62:.control,57:.capsLock]
}
