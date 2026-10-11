import AppKit
import QuartzCore
import SwiftUI
import VnilinkCore

enum DesktopSizing: Int {
    case fit, actualSize
}

@MainActor final class ViewerModel: NSObject, ObservableObject {
    @Published var session: FfiSessionInfo?
    @Published var picture: CGImage?
    @Published var sizing: DesktopSizing = .fit
    @Published var separateCursor = false
    @Published var cursor: MacCursorUpdate?
    private(set) var cursorRevision: UInt64 = 0
    @Published var status = "Connecting…"
    @Published var failure: String?
    @Published var code: String?
    @Published var controlling = true {
        didSet {
            guard oldValue != controlling, let desktop else { return }
            let desired = controlling
            inputQueue.async {
                if desired { try? desktop.acquireControl() }
                else { try? desktop.yieldControl() }
            }
        }
    }
    private(set) var desktopInfo: FfiDesktopInfo?
    private(set) var frameSourceId: UInt64 = 0
    private(set) var frameSourceRevision: UInt64 = 0
    private(set) var desktop: DesktopService?
    private let backend: ServiceDesktopBackend
    @Published var notice: String?
    let id = UUID()
    let name: String
    let deviceId: String
    private var sessionId: UInt64?
    private var sessionTimer: Timer?
    private let core: VnilinkCore
    private var filesWindow: FilesWindowController?
    private let videoQueue = DispatchQueue(label: "com.vnilink.viewer.video", qos: .userInitiated)
    private let inputQueue = DispatchQueue(label: "com.vnilink.viewer.input", qos: .userInteractive)
    private let connectionQueue = DispatchQueue(label: "com.vnilink.viewer.connection", qos: .userInitiated)
    private var displayLink: CADisplayLink?
    private var poller: MacViewer?
    private var updating = false
    private var closed = false
    private var visible = false
    private var epoch: UInt64 = 0
    private var decodingGeneration: UInt64?
    private var pendingInput = 0
    private var nextSessionCheck = 0.0

    init(core: VnilinkCore, device: FfiDeviceInfo, preview: Bool = false) {
        self.core = core; backend = ServiceDesktopBackend(core: core); name = device.name; deviceId = device.id
        super.init()
        if preview {
            status = "Disconnected"; failure = "Preview session"; notice = "Preview only · no remote connection"; controlling = false
            return
        }
        connectionQueue.async {
            let result = Result {
                try core.beginConnectServices(endpointId: device.id)
            }
            DispatchQueue.main.async {
                switch result {
                case .success(let session):
                    self.sessionId = session.id
                    if self.closed { self.backend.close() }
                    else { self.session = session; self.update() }
                case .failure(let error):
                    if !self.closed { self.failure = AppModel.message(error); self.status = "Connection failed" }
                }
            }
        }
        // Session ownership outlives the selected page and its display link.
        sessionTimer = Timer.scheduledTimer(withTimeInterval: 0.1, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.update() }
        }
    }
    func startUpdates(in view: NSView) {
        guard !closed else { return }
        pausePresentation()
        let link = view.displayLink(target: self, selector: #selector(displayTick(_:)))
        link.preferredFrameRateRange = CAFrameRateRange(minimum: 30, maximum: 60, preferred: 60)
        link.add(to: .main, forMode: .common)
        displayLink = link
    }
    func pausePresentation() { displayLink?.invalidate(); displayLink = nil }
    @objc private func displayTick(_ link: CADisplayLink) {
        guard !closed else { return }
        if let update = poller?.pollCursor() { cursorRevision &+= 1; cursor = update }
        // Polling only takes ownership of the latest decoded frame; no pixel copy or decode.
        if let session, session.phase == .active, let frame = poller?.poll() {
            picture = Self.image(frame)
            frameSourceId = frame.displayId()
            frameSourceRevision = frame.revision()
        }
        // Session snapshots run off-thread and do not delay the display cadence.
        if link.timestamp >= nextSessionCheck {
            nextSessionCheck = link.timestamp + 0.1
            update()
        }
    }
    private func update() {
        guard !closed else { return }
        guard !updating else { return }
        updating = true
        guard let sessionId else { updating = false; return }
        let epoch = epoch
        let poller = poller
        videoQueue.async {
            let result = Result { (try self.core.controllerSession(sessionId: sessionId), self.core.controlCheck(), poller?.error()) }
            DispatchQueue.main.async {
                self.updating = false
                guard !self.closed, self.epoch == epoch else { return }
                switch result {
                case .failure(let error): self.failure = AppModel.message(error)
                case .success(let data):
                    self.session = data.0
                    switch data.0.phase {
                    case .connecting, .authenticated, .authorized, .negotiating: self.code = data.1
                    default: self.code = nil
                    }
                    if let error = data.2 { self.failure = error }
                    let session = data.0
                    self.status = Self.label(session.phase)
                    if session.phase == .active {
                        if self.decodingGeneration != session.generation {
                            self.startDecoder(session)
                        }
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
            let result = Result {
                let desktop = try self.backend.open(session)
                let info = try desktop.desktopInfo()
                let poller = try self.core.macDesktopViewer(desktop: desktop)
                return (desktop, info, poller)
            }
            DispatchQueue.main.async {
                guard !self.closed, epoch == self.epoch else {
                    if case .success(let data) = result { self.videoQueue.async { data.2.stop(); data.0.close() } }
                    return
                }
                switch result {
                case .success(let data):
                    self.desktop = data.0; self.desktopInfo = data.1
                    self.poller = data.2; self.separateCursor = data.2.separateCursor()
                    if self.controlling { self.inputQueue.async { try? data.0.acquireControl() } }
                case .failure(let error): self.failure = AppModel.message(error)
                }
            }
        }
    }
    private func stopDecoder() {
        epoch += 1
        let old = poller; poller = nil; decodingGeneration = nil; picture = nil; separateCursor = false; cursor = nil
        desktop = nil; desktopInfo = nil; frameSourceId = 0; frameSourceRevision = 0
        backend.retire()
        videoQueue.async { old?.stop() }
    }
    func send(release: Bool = false, _ operation: @escaping (DesktopService) throws -> Void) {
        guard !closed, ((controlling && visible) || release), let desktop, !desktop.isCancelled() else { return }
        guard pendingInput < 128 else { failure = "Input fell behind. Reconnect to continue."; close(); return }
        pendingInput += 1
        inputQueue.async {
            let result = Result {
                guard try release || desktop.controlState() == .controlling else { return }
                try operation(desktop)
            }
            DispatchQueue.main.async {
                self.pendingInput -= 1
                if case .failure(let error) = result, !self.closed {
                    // A reconnect can invalidate already queued input. Discard
                    // it rather than closing the replacement connection.
                    if case FfiError.StaleSession = error { return }
                    if case FfiError.NoSession = error { return }
                    if case FfiError.Unauthorized = error { self.notice = "Viewing only: control is not granted."; return }
                    if case FfiError.Busy = error { self.notice = "Control belongs to another viewer."; return }
                    self.failure = AppModel.message(error); self.close()
                }
            }
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
        guard let desktop, !desktop.isCancelled() else { notice = "Connect to a computer first."; return }
        guard let text = NSPasteboard.general.string(forType: .string) else { notice = "The clipboard does not contain text."; return }
        action({ _ in try desktop.submitClipboard(text: text) }, success: "Clipboard text sent.")
    }
    func sendKey(_ key: DesktopKey, release: Bool = false, paste: Bool = false) {
        let clipboard = paste ? NSPasteboard.general.string(forType: .string) : nil
        send(release: release) { desktop in
            if let clipboard {
                // The clipboard stream acknowledges the host write before V is queued.
                // Keep input usable if clipboard has not been granted separately.
                do { try desktop.submitClipboard(text: clipboard) }
                catch { DispatchQueue.main.async { self.notice = AppModel.message(error) } }
            }
            switch key.route {
            case .local:
                try desktop.submitLocalKey(
                    usage: key.usage, down: key.down, text: key.text)
            case .physical:
                try desktop.submitKey(
                    usage: key.usage, down: key.down)
            }
        }
    }
    func sendFile(_ url: URL) {
        guard session?.phase == .active else { return }
        openFiles()
        filesWindow?.selectUpload(url)
    }
    func openFiles() {
        if let filesWindow, filesWindow.window?.isVisible == true {
            filesWindow.window?.makeKeyAndOrderFront(nil)
            return
        }
        let core = core, peer = deviceId
        let files = FilesWindowController.open(core: core, session: {
            try? core.controllerSessions().first { $0.peerId == peer && $0.phase == .active }
        }, peerId: deviceId, localName: Host.current().localizedName ?? "This Mac", remoteName: name)
        filesWindow = files
        files.showWindow(nil)
        files.window?.makeKeyAndOrderFront(nil)
    }
    func setVisible(_ visible: Bool) {
        guard self.visible != visible else { return }
        self.visible = visible
        if !visible { releaseInput() }
    }
    func releaseInput() {
        guard !closed, let desktop, !desktop.isCancelled() else { return }
        inputQueue.async {
            // Run after previously queued key/button events; retry a busy core queue.
            for _ in 0..<20 {
                do { try desktop.releaseInput(); return }
                catch FfiError.Busy { Thread.sleep(forTimeInterval: 0.01) }
                catch { return }
            }
        }
    }
    func close() {
        guard !closed else { return }
        closed = true; filesWindow = nil; backend.close(); sessionTimer?.invalidate(); sessionTimer = nil
        displayLink?.invalidate(); displayLink = nil; stopDecoder(); status = "Disconnected"; code = nil; session = nil
        // The peer connection remains available to independent service owners.
    }
    static func image(_ frame: MacFrame) -> CGImage? {
        let buffer = frame.buffer()
        guard let pixels = UnsafeRawPointer(bitPattern: UInt(buffer.address)),
              let byteCount = Int(exactly: buffer.byteCount) else { return nil }
        // CGImage may outlive the latest-frame slot. Its provider owns one
        // retained Swift frame wrapper, which in turn owns the Rust pixels.
        let owner = Unmanaged.passRetained(frame).toOpaque()
        guard let provider = CGDataProvider(dataInfo: owner, data: pixels, size: byteCount,
            releaseData: { owner, _, _ in
                if let owner { Unmanaged<MacFrame>.fromOpaque(owner).release() }
            }) else {
            Unmanaged<MacFrame>.fromOpaque(owner).release()
            return nil
        }
        return CGImage(width: Int(buffer.width), height: Int(buffer.height), bitsPerComponent: 8, bitsPerPixel: 32,
                       bytesPerRow: Int(buffer.stride), space: CGColorSpaceCreateDeviceRGB(),
                       bitmapInfo: [.byteOrder32Little, CGBitmapInfo(rawValue: CGImageAlphaInfo.noneSkipFirst.rawValue)],
                       provider: provider, decode: nil, shouldInterpolate: false, intent: .defaultIntent)
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

@MainActor final class ViewerPageController: NSHostingController<ViewerContent> {
    private let model: ViewerModel
    init(model: ViewerModel) {
        self.model = model
        super.init(rootView: ViewerContent(model: model))
    }
    @MainActor required dynamic init?(coder: NSCoder) { fatalError("init(coder:) is unavailable") }
    override func viewDidAppear() { super.viewDidAppear(); model.setVisible(true); model.startUpdates(in: view) }
    override func viewDidDisappear() { model.setVisible(false); model.pausePresentation(); super.viewDidDisappear() }
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
                    DesktopCanvas(model: model, image: picture, sizing: model.sizing, separateCursor: model.separateCursor, cursor: model.cursor, cursorRevision: model.cursorRevision)
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
    let sizing: DesktopSizing
    let separateCursor: Bool
    let cursor: MacCursorUpdate?
    let cursorRevision: UInt64
    func makeNSView(context: Context) -> DesktopScrollView { DesktopScrollView(model: model) }
    func updateNSView(_ view: DesktopScrollView, context: Context) {
        view.desktop.image = image
        view.desktop.controlling = model.controlling
        let changed = view.desktop.sizing != sizing
        view.desktop.sizing = sizing
        view.updateDocumentSize()
        if changed {
            view.contentView.scroll(to: .zero)
            view.reflectScrolledClipView(view.contentView)
        }
        view.desktop.updateCursor(cursor, revision: cursorRevision, separate: separateCursor)
        view.desktop.needsDisplay = true
    }
}

final class DesktopScrollView: NSScrollView {
    let desktop: DesktopView
    init(model: ViewerModel) {
        desktop = DesktopView(model: model)
        super.init(frame: .zero)
        drawsBackground = false
        hasHorizontalScroller = true; hasVerticalScroller = true
        autohidesScrollers = true
        documentView = desktop
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) is unavailable") }
    override func layout() {
        super.layout()
        updateDocumentSize()
    }
    override func viewDidChangeBackingProperties() {
        super.viewDidChangeBackingProperties()
        updateDocumentSize()
    }
    func updateDocumentSize() {
        var size = contentView.bounds.size
        if desktop.sizing == .actualSize, let image = desktop.image {
            let scale = window?.backingScaleFactor ?? 1
            size.width = max(size.width, CGFloat(image.width) / scale)
            size.height = max(size.height, CGFloat(image.height) / scale)
        }
        if desktop.frame.size != size { desktop.setFrameSize(size) }
        desktop.needsDisplay = true
    }
}

final class DesktopView: NSView {
    private let model: ViewerModel
    var image: CGImage?
    var sizing: DesktopSizing = .fit
    private var separateCursor = false
    private var cursorVisible = true
    private var cursorPixels: CGImage?
    private var cursorHotspot = NSPoint.zero
    private var cursorPosition = NSPoint.zero
    private var localCursorHidden = false
    private var cursorRevision: UInt64?
    private var nativeCursor: NSCursor?
    private var nativeCursorScale: CGFloat = 0
    var controlling = true {
        didSet {
            if oldValue != controlling { window?.invalidateCursorRects(for: self) }
            if oldValue != controlling && !controlling {
                releaseInput()
                showLocalCursor()
            }
        }
    }
    private var keyboard = DesktopKeyboard()
    private var buttons = Set<Int>()
    /// Pointer source bound to the displayed frame. The host streams exactly
    /// the display chosen at share time and reports its size in the accept
    /// handshake; the source is the unique topology display matching that
    /// geometry, never the first list item. Keyed by owner identity so a
    /// replacement owner reselects, and by topology revision so hotplug
    /// rebinds. Confined to the input queue, which is serial.
    private var pointerSourceOwner: ObjectIdentifier?
    private var pointerSourceId: UInt64?
    private var pointerSourceRevision: UInt64?
    private var tracking: NSTrackingArea?
    private var inactiveObserver: NSObjectProtocol?
    init(model: ViewerModel) {
        self.model = model; super.init(frame: .zero)
        registerForDraggedTypes([.fileURL])
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) is unavailable") }
    override var acceptsFirstResponder: Bool { true }
    override var isFlipped: Bool { true }
    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        if controlling, window?.firstResponder == nil || window?.firstResponder === window {
            window?.makeFirstResponder(self)
        }
    }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { controlling }
    override func draggingEntered(_ sender: NSDraggingInfo) -> NSDragOperation {
        guard model.session?.phase == .active,
              let urls = sender.draggingPasteboard.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL], urls.count == 1 else { return [] }
        return .copy
    }
    override func performDragOperation(_ sender: NSDraggingInfo) -> Bool {
        guard draggingEntered(sender) == .copy,
              let urls = sender.draggingPasteboard.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL], let url = urls.first else { return false }
        model.sendFile(url); return true
    }
    var imageRect: NSRect {
        guard let image else { return .zero }
        let scale = sizing == .actualSize
            ? 1 / (window?.backingScaleFactor ?? 1)
            : min(bounds.width / CGFloat(image.width), bounds.height / CGFloat(image.height))
        let size = NSSize(width: CGFloat(image.width) * scale, height: CGFloat(image.height) * scale)
        var rect = NSRect(x: (bounds.width-size.width)/2, y: (bounds.height-size.height)/2, width: size.width, height: size.height)
        if sizing == .actualSize {
            // Keep the 1:1 image on whole backing pixels, including its centered origin.
            var pixels = convertToBacking(rect)
            pixels.origin.x = pixels.origin.x.rounded(); pixels.origin.y = pixels.origin.y.rounded()
            rect = convertFromBacking(pixels)
        }
        return rect
    }
    func updateCursor(_ update: MacCursorUpdate?, revision: UInt64, separate: Bool) {
        let changed = separateCursor != separate || cursorRevision != revision
        var appearanceChanged = separateCursor != separate
        separateCursor = separate
        if separate { showLocalCursor() }
        if changed, let update {
            cursorRevision = revision
            appearanceChanged = appearanceChanged || cursorVisible != update.visible || update.image != nil
            cursorVisible = update.visible
            cursorPosition = NSPoint(x: CGFloat(update.x), y: CGFloat(update.y))
            if let bitmap = update.image {
                let data = Data(bitmap.bgra)
                if let provider = CGDataProvider(data: data as CFData) {
                    cursorPixels = CGImage(width: Int(bitmap.width), height: Int(bitmap.height), bitsPerComponent: 8, bitsPerPixel: 32,
                        bytesPerRow: Int(bitmap.width)*4, space: CGColorSpaceCreateDeviceRGB(),
                        bitmapInfo: [.byteOrder32Little, CGBitmapInfo(rawValue: CGImageAlphaInfo.premultipliedFirst.rawValue)],
                        provider: provider, decode: nil, shouldInterpolate: false, intent: .defaultIntent)
                    cursorHotspot = NSPoint(x: CGFloat(bitmap.hotspotX), y: CGFloat(bitmap.hotspotY))
                }
            }
        }
        let scale = image.map { imageRect.width / CGFloat($0.width) } ?? 0
        if appearanceChanged || nativeCursorScale != scale {
            nativeCursor = nil; nativeCursorScale = scale
            window?.invalidateCursorRects(for: self)
        }
    }
    private func showLocalCursor() {
        if localCursorHidden { NSCursor.unhide(); localCursorHidden = false }
    }
    private func remoteCursor() -> NSCursor {
        if let nativeCursor { return nativeCursor }
        guard cursorVisible else {
            return NSCursor(image: NSImage(size: NSSize(width: 1, height: 1), flipped: false) { _ in true }, hotSpot: .zero)
        }
        guard let cursorPixels, let image else { return .arrow }
        let scale = imageRect.width / CGFloat(image.width)
        let cursor = NSCursor(image: NSImage(cgImage: cursorPixels, size: NSSize(width: CGFloat(cursorPixels.width)*scale, height: CGFloat(cursorPixels.height)*scale)),
            hotSpot: NSPoint(x: cursorHotspot.x*scale, y: cursorHotspot.y*scale))
        nativeCursor = cursor
        return cursor
    }
    override func resetCursorRects() {
        super.resetCursorRects()
        if separateCursor && controlling { addCursorRect(imageRect, cursor: remoteCursor()) }
    }
    override func draw(_ dirtyRect: NSRect) {
        guard let image, let context = NSGraphicsContext.current?.cgContext else { return }
        let rect = imageRect
        context.saveGState(); context.translateBy(x: rect.minX, y: rect.maxY); context.scaleBy(x: 1, y: -1)
        context.interpolationQuality = sizing == .actualSize ? .none : .high
        context.draw(image, in: CGRect(origin: .zero, size: rect.size)); context.restoreGState()
        if separateCursor && !controlling && cursorVisible, let cursorPixels {
            let scale = rect.width / CGFloat(image.width)
            let cursorRect = NSRect(x: rect.minX + (cursorPosition.x-cursorHotspot.x)*scale,
                y: rect.minY + (cursorPosition.y-cursorHotspot.y)*scale,
                width: CGFloat(cursorPixels.width)*scale, height: CGFloat(cursorPixels.height)*scale)
            context.saveGState(); context.clip(to: rect)
            context.translateBy(x: cursorRect.minX, y: cursorRect.maxY); context.scaleBy(x: 1, y: -1)
            context.draw(cursorPixels, in: CGRect(origin: .zero, size: cursorRect.size)); context.restoreGState()
        }
    }
    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let tracking { removeTrackingArea(tracking) }
        tracking = NSTrackingArea(rect: .zero, options: [.mouseMoved, .mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect], owner: self)
        addTrackingArea(tracking!)
    }
    override func mouseEntered(with event: NSEvent) {
        // Remote cursor is baked into the frame. Hide the local cursor over
        // the video so only one pointer is visible.
        if controlling {
            if separateCursor { remoteCursor().set() }
            else if !localCursorHidden { NSCursor.hide(); localCursorHidden = true }
        }
    }
    override func mouseExited(with event: NSEvent) {
        showLocalCursor()
    }
    override func resignFirstResponder() -> Bool { releaseInput(); return super.resignFirstResponder() }
    override func viewWillMove(toWindow newWindow: NSWindow?) {
        releaseInput()
        showLocalCursor()
        if let inactiveObserver { NotificationCenter.default.removeObserver(inactiveObserver) }
        inactiveObserver = nil
        if let newWindow {
            inactiveObserver = NotificationCenter.default.addObserver(forName: NSWindow.didResignKeyNotification, object: newWindow, queue: .main) { [weak self] _ in
                self?.releaseInput(); self?.showLocalCursor()
            }
        }
        super.viewWillMove(toWindow: newWindow)
    }
    deinit { if let inactiveObserver { NotificationCenter.default.removeObserver(inactiveObserver) } }
    private func pointer(_ event: NSEvent) -> Bool {
        guard controlling, let desktop = model.desktopInfo, desktop.monitorWidth > 0, desktop.monitorHeight > 0 else { return false }
        let point = convert(event.locationInWindow, from: nil), rect = imageRect
        guard let (fractionX, fractionY) = PointerSource.fraction(point: point, in: rect) else { return false }
        let frameWidth = desktop.monitorWidth, frameHeight = desktop.monitorHeight
        let frameSourceId = model.frameSourceId, frameSourceRevision = model.frameSourceRevision
        model.send { desktop in
            // The host rejects pointer input until this owner selects the
            // display it names. Select the frame's stamped source before
            // sending; a missing, ambiguous or revision-mismatched source
            // fences the pointer until matching pixels arrive. The picture
            // fraction scales onto the selected source's extent, never
            // onto the handshake monitor.
            guard let source = PointerSource.select(
                displays: try desktop.topology(),
                frameDisplayId: frameSourceId, frameRevision: frameSourceRevision,
                frameWidth: frameWidth, frameHeight: frameHeight) else { return }
            let owner = ObjectIdentifier(desktop)
            if self.pointerSourceOwner != owner || self.pointerSourceId != source.id
                || self.pointerSourceRevision != source.revision {
                try desktop.selectDisplays(ids: [source.id])
                self.pointerSourceOwner = owner
                self.pointerSourceId = source.id
                self.pointerSourceRevision = source.revision
            }
            let (x, y) = source.normalize(fractionX: fractionX, fractionY: fractionY)
            guard let (mappedX, mappedY) = source.map(x: x, y: y) else { return }
            try desktop.submitPointer(
                x: mappedX, y: mappedY,
                display: source.id, revision: source.revision)
        }
        return true
    }
    override func mouseMoved(with event: NSEvent) { _ = pointer(event) }
    override func mouseDragged(with event: NSEvent) { _ = pointer(event) }
    override func rightMouseDragged(with event: NSEvent) { _ = pointer(event) }
    override func otherMouseDragged(with event: NSEvent) { _ = pointer(event) }
    private func button(_ event: NSEvent, down: Bool) {
        guard (0...4).contains(event.buttonNumber) else { return }
        if down {
            guard pointer(event) else { return }
            window?.makeFirstResponder(self); buttons.insert(event.buttonNumber)
        } else { guard buttons.remove(event.buttonNumber) != nil else { return } }
        let button: FfiPointerButton = event.buttonNumber == 0 ? .left : event.buttonNumber == 1 ? .right : event.buttonNumber == 2 ? .middle : event.buttonNumber == 3 ? .back : .forward
        model.send { try $0.submitButton(button: button, down: down) }
    }
    override func mouseDown(with event: NSEvent) { button(event, down: true) }
    override func mouseUp(with event: NSEvent) { button(event, down: false) }
    override func rightMouseDown(with event: NSEvent) { button(event, down: true) }
    override func rightMouseUp(with event: NSEvent) { button(event, down: false) }
    override func otherMouseDown(with event: NSEvent) { button(event, down: true) }
    override func otherMouseUp(with event: NSEvent) { button(event, down: false) }
    private var scrollRemainder = (x: 0.0, y: 0.0)
    override func scrollWheel(with event: NSEvent) {
        guard pointer(event) else { return }
        // AppKit precise deltas are points; ordinary wheel deltas are lines.
        let scale = event.hasPreciseScrollingDeltas ? 12.0 : 120.0
        func convert(_ delta: Double, remainder: inout Double) -> Int32 {
            guard delta.isFinite else { return 0 }
            let total = max(-7680.0, min(7680.0, delta * scale + remainder))
            let whole = total.rounded(.towardZero)
            remainder = total - whole
            return Int32(whole)
        }
        let dx = convert(event.scrollingDeltaX, remainder: &scrollRemainder.x)
        let dy = convert(event.scrollingDeltaY, remainder: &scrollRemainder.y)
        guard dx != 0 || dy != 0 else { return }
        model.send { try $0.submitPreciseWheel(dx: dx, dy: dy) }
    }
    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        guard controlling, window?.firstResponder === self, event.modifierFlags.contains(.command) else { return false }
        // Tab/window navigation remains local even when the desktop has focus.
        let character = event.charactersIgnoringModifiers?.lowercased()
        if character == "w" || (event.modifierFlags.contains(.shift) && ["[", "]", "{", "}"].contains(character ?? "")) { return false }
        keyDown(with: event); return true
    }
    private func sendKey(_ key: DesktopKey, release: Bool = false) {
        model.sendKey(key, release: release)
    }
    override func keyDown(with event: NSEvent) {
        guard controlling else { return }
        for modifier in keyboard.synchronize(event.modifierFlags) { sendKey(modifier) }
        guard let key = keyboard.key(code: event.keyCode, down: true, text: event.characters ?? "") else { return }
        let paste = key.usage == 25 && !event.modifierFlags.contains(.option)
            && !event.modifierFlags.intersection([.command, .control]).isEmpty
        model.sendKey(key, paste: paste)
    }
    override func keyUp(with event: NSEvent) {
        guard let key = keyboard.key(code: event.keyCode, down: false) else { return }
        sendKey(key, release: true)
    }
    override func flagsChanged(with event: NSEvent) {
        guard controlling, let key = keyboard.modifier(code: event.keyCode, flags: event.modifierFlags) else { return }
        sendKey(key)
    }
    private func releaseInput() {
        scrollRemainder = (0, 0)
        for key in keyboard.release() { sendKey(key, release: true) }
        for number in buttons {
            let button: FfiPointerButton = number == 0 ? .left : number == 1 ? .right : number == 2 ? .middle : number == 3 ? .back : .forward
            model.send(release: true) { try $0.submitButton(button: button, down: false) }
        }
        buttons.removeAll()
    }
}
